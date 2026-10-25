#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";

import fs from "node:fs";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";
import { assertRendererConsoleClean, browserConsoleEvidence } from "../../tools/browser_console_evidence.mjs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { parseRealtimeWeaponReceipt, sameDecoyEntity } from "../../tools/realtime_event_evidence.mjs";

import {
  assertDataChannelDeliveryAudit,
  assertSecurityRejectAudit,
  authoredDoorClosedForCurrentAndNext,
  brakeClientVelocity,
  browserE2eFailureFooter,
  compactMetrics,
  driveClientToWorldTarget,
  openChromeClient,
  saveBrowserE2eArtifacts,
  startChrome,
  summarizeClient,
  waitForChrome,
  waitForPlayersReady,
  waitUntil,
} from "./combat_replay.mjs";
import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { assertRetainedArtifactDirectoryAvailable, validateArtifactPathReferences } from "./artifact_paths.mjs";
import {
  createSession,
  deleteSession,
  execute as executeFirefox,
  firefoxArtifactClient,
  freePort,
  openClient as openFirefoxClient,
  metricsState as firefoxMetricsState,
  repoRoot,
  sleep,
  startGeckodriver,
  startRealServer,
  serverBinaryForProfile,
  startStaticServer,
  stopProcess,
  waitForGeckodriver,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {
  parseServerProjectileEvents,
  parseZubspaceEvents,
} from "../../tools/server_events.mjs";

const applicationRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const wasmPath = process.env.ZUBSPACE_E2E_STATIC_ROOT
  ? path.relative(repoRoot, path.join(applicationRoot, "zubspace.wasm"))
  : "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const clientEntry = path.relative(repoRoot, path.join(applicationRoot, "index.html"));
const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
// Chaos exercises production maximum-entry actions. Extreme retains the
// explicitly authored-zero admission fixture, independently of product entry.
const maxedSpawnLoadout = arena !== "extreme";
const browserPair = process.env.ZUBSPACE_E2E_BROWSER_PAIR || "chrome-chrome";
const firefoxActor = browserPair === "chrome-firefox";
const forcedRealtimeRepelReorder =
  process.env.ZUBSPACE_E2E_REALTIME_REPEL_REORDER === "1";
const successArtifactDir = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR || "";
assertRetainedArtifactDirectoryAvailable(successArtifactDir);
const runStartedAt = new Date().toISOString();
const e2eScript = "tests/e2e/special_weapons.mjs";
const serverProfile = process.env.ZUBSPACE_E2E_SERVER_PROFILE || "debug";
const serverBinary = serverBinaryForProfile(serverProfile);
// A Firefox execute-script readiness sample and the following actions command
// are separate Marionette tasks on the renderer main thread. Yield one frozen
// 25 ms presentation-recovery frame plus one 10 ms policy tick inside the same
// genuine WebDriver action before keydown. This prevents the diagnostic RPC
// itself from manufacturing the scheduler hold being tested; no gameplay
// packet is injected and a failed physical edge is never retried.
const FIREFOX_PREPARED_ACTION_SETTLE_MS = 35;

if (!["chaos-zone", "extreme"].includes(arena)) {
  throw new Error("special-weapon fidelity replay is scoped to chaos-zone and extreme");
}
if (!["chrome-chrome", "chrome-firefox"].includes(browserPair)) {
  throw new Error(`unsupported browser pair ${browserPair}`);
}
if (forcedRealtimeRepelReorder && arena !== "chaos-zone") {
  throw new Error("forced realtime Repel reorder is scoped to chaos-zone");
}

async function webdriver(driver, method, pathname, body = undefined) {
  const response = await fetch(`http://127.0.0.1:${driver.port}${pathname}`, {
    method,
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  const parsed = text ? JSON.parse(text) : {};
  if (!response.ok) {
    throw new Error(`${driver.label} WebDriver ${method} ${pathname} failed ${response.status}: ${text}`);
  }
  return parsed.value;
}

function firefoxKeyValue(key) {
  const keys = {
    Escape: "\uE00C",
    Shift: "\uE008",
    Space: "\uE00D",
    ArrowLeft: "\uE012",
    ArrowUp: "\uE013",
    ArrowRight: "\uE014",
    ArrowDown: "\uE015",
    Insert: "\uE016",
    Delete: "\uE017",
    F3: "\uE033",
    F5: "\uE035",
    F6: "\uE036",
  };
  return keys[key] || key;
}

async function performFirefoxKeyActions(driver, actions) {
  await webdriver(driver, "POST", `/session/${driver.sessionId}/actions`, {
    actions: [{ type: "key", id: "keyboard", actions }],
  });
  await webdriver(driver, "DELETE", `/session/${driver.sessionId}/actions`);
}

async function recordFirefoxInput(driver, phase, key) {
  const payload = JSON.stringify({ phase, key });
  await executeFirefox(driver, `
    const input = ${payload};
    const metrics = window.__zubspaceUiMetrics || {};
    window.__zubspaceInputTrace ||= [];
    window.__zubspaceInputTrace.push({
      at: Date.now(),
      label: "special-firefox",
      ...input,
      selfPlayerId: metrics.selfPlayerId,
      clientInputSeq: metrics.clientInputSeq,
      latestServerTick: metrics.latestServerTick,
      semanticInputCounts: metrics.semanticInputCounts,
      localGameplayState: metrics.localGameplayState,
      combatViewTimeline: metrics.combatViewTimeline,
    });
    return true;
  `).catch(() => {});
}

async function openFirefoxSpecialClient(driver, url) {
  await openFirefoxClient(driver, url);
  return {
    label: driver.label,
    async focus() {
      await executeFirefox(driver, "document.querySelector('canvas')?.focus(); return true;");
    },
    async press(key, holdMs = 80) {
      await this.focus();
      const value = firefoxKeyValue(key);
      await recordFirefoxInput(driver, "keyDown", key);
      await performFirefoxKeyActions(driver, [
        { type: "keyDown", value },
        { type: "pause", duration: Math.max(0, Math.round(holdMs)) },
        { type: "keyUp", value },
      ]);
      await recordFirefoxInput(driver, "keyUp", key);
    },
    async pressPrepared(key, holdMs = 80) {
      const value = firefoxKeyValue(key);
      await performFirefoxKeyActions(driver, [
        { type: "pause", duration: FIREFOX_PREPARED_ACTION_SETTLE_MS },
        { type: "keyDown", value },
        { type: "pause", duration: Math.max(0, Math.round(holdMs)) },
        { type: "keyUp", value },
      ]);
    },
    async hold(key, holdMs = 500) {
      await this.press(key, holdMs);
    },
    async chord(modifier, key, modifierHoldMs = 55) {
      await this.focus();
      const modifierValue = firefoxKeyValue(modifier);
      const keyValue = firefoxKeyValue(key);
      await recordFirefoxInput(driver, "keyDown", modifier);
      await recordFirefoxInput(driver, "keyDown", key);
      await performFirefoxKeyActions(driver, [
        { type: "keyDown", value: modifierValue },
        { type: "keyDown", value: keyValue },
        { type: "pause", duration: modifierHoldMs },
        { type: "keyUp", value: keyValue },
        { type: "keyUp", value: modifierValue },
      ]);
      await recordFirefoxInput(driver, "keyUp", key);
      await recordFirefoxInput(driver, "keyUp", modifier);
    },
    async chordPrepared(modifier, key, modifierHoldMs = 55) {
      const modifierValue = firefoxKeyValue(modifier);
      const keyValue = firefoxKeyValue(key);
      await performFirefoxKeyActions(driver, [
        { type: "pause", duration: FIREFOX_PREPARED_ACTION_SETTLE_MS },
        { type: "keyDown", value: modifierValue },
        { type: "keyDown", value: keyValue },
        { type: "pause", duration: modifierHoldMs },
        { type: "keyUp", value: keyValue },
        { type: "keyUp", value: modifierValue },
      ]);
    },
    async enterShip() {
      for (let attempt = 0; attempt < 4; attempt += 1) {
        await this.press("Escape", 60);
        await this.press("2", 50);
        await sleep(300);
      }
    },
    ...firefoxArtifactClient(driver),
    async metricsState() {
      return firefoxMetricsState(driver);
    },
    async screenshot() {
      return webdriver(driver, "GET", `/session/${driver.sessionId}/screenshot`);
    },
    async close() {
      await deleteSession(driver);
    },
  };
}

function localState(state) {
  return state.uiMetrics?.localGameplayState || {};
}

function semanticDelta(before, after, key) {
  return (after?.[key] ?? 0) - (before?.[key] ?? 0);
}

async function waitForWeaponReady(client, label) {
  await waitUntil(`${label} weapon clocks and lifecycle ready`, async () => {
    const metrics = (await client.metricsState()).uiMetrics || {};
    const local = metrics.localGameplayState || {};
    return (
      metrics.playerAlive === true &&
      metrics.serverPlayerAlive === true &&
      metrics.inSafeZone === false &&
      local.doorClockFresh === true &&
      local.pendingShipResetDeliveryId == null &&
      local.pendingFrequencyBaselineDeliveryId == null &&
      metrics.combatViewTimeline?.weaponReady === true &&
      (local.bombCooldown ?? 1) <= 0 &&
      (local.burstCooldown ?? 1) <= 0 &&
      (local.repelCooldown ?? 1) <= 0
    );
  }, 12_000);
}

async function armWeaponInput(client, label) {
  // Firefox's Marionette execute-script calls can stall a busy renderer long
  // enough for a previously sampled combat view to become held. Establish
  // focus first, then make the readiness read the final command before the
  // genuine WebDriver action.
  await client.focus();
  await waitForWeaponReady(client, label);
}

async function pressArmed(client, key, holdMs = 80) {
  if (typeof client.pressPrepared === "function") {
    await client.pressPrepared(key, holdMs);
    return;
  }
  await client.press(key, holdMs);
}

async function chordArmed(client, modifier, key, modifierHoldMs = 55) {
  if (typeof client.chordPrepared === "function") {
    await client.chordPrepared(modifier, key, modifierHoldMs);
    return;
  }
  await client.chord(modifier, key, modifierHoldMs);
}

async function settleSpecialWeaponLane(
  client,
  target,
  {
    routeTimeoutMs,
    maximumDesiredSpeed,
    desiredSpeedScale,
    progressTimeoutMs,
    attempts = 2,
  },
) {
  const evidence = [];
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    const route = await driveClientToWorldTarget(
      client,
      target,
      routeTimeoutMs,
      true,
      8,
      null,
      maximumDesiredSpeed,
      desiredSpeedScale,
      progressTimeoutMs,
    );
    const brake = await brakeClientVelocity(client, 5_000);
    const metrics = (await client.metricsState()).uiMetrics || {};
    const distance = Math.hypot(
      metrics.playerX - target.x,
      metrics.playerY - target.y,
    );
    const sample = {
      attempt,
      route: route.reached
        ? route
        : {
            reached: false,
            reason: route.reason,
            bestDistance: route.bestDistance,
            bestNearTargetSpeed: route.bestNearTargetSpeed,
            pulses: route.pulses,
            target: route.target,
            final: compactMetrics(route.final),
          },
      brake: brake.braked
        ? { braked: true, speed: brake.speed }
        : { braked: false, reason: brake.reason },
      distance,
      position: { x: metrics.playerX, y: metrics.playerY },
      velocity: { x: metrics.playerVelocityX, y: metrics.playerVelocityY },
    };
    evidence.push(sample);
    if (brake.braked && distance <= 8) {
      return { reached: true, ...sample, evidence };
    }
  }
  return { reached: false, target, evidence };
}

async function waitForZoneReady(client, label) {
  // Do not race ship selection against the file/settings/map lifecycle. A
  // headed Firefox or one of three busy Chrome renderers can otherwise consume
  // every short "2" edge while gameplay input is intentionally closed and
  // remain a spectator forever.
  await waitUntil(`${label} zone ready for ship selection`, async () => {
    const local = (await client.metricsState()).uiMetrics?.localGameplayState || {};
    return local.zoneFilesReady === true && local.zoneMapReady === true;
  }, 45_000);
}

async function enterReadyShip(client, label) {
  await waitUntil(`${label} real 2 enters a live ship`, async () => {
    const metrics = (await client.metricsState()).uiMetrics || {};
    if (metrics.playerLifeSeq != null && metrics.serverPlayerEnergy != null) return true;
    await client.press("Escape", 60);
    await client.press("2", 160);
    await sleep(350);
    return false;
  }, 45_000);
}


function newProjectileSpawns(server, beforeCount, ownerId, weapon) {
  return serverProjectileEvents(server)
    .filter((event) => event.type === "server_projectile_spawned")
    .slice(beforeCount)
    .filter((event) => event.ownerId === ownerId && (event.weapon & 0x1f) === weapon);
}

function remoteProjectile(metrics, playerId, kind) {
  return (metrics?.remoteProjectileSources || []).find((projectile) =>
    projectile.playerId === playerId && projectile.kind === kind
  ) || null;
}

function remoteDecoy(metrics, playerId) {
  return (metrics?.remoteDecoys || []).find((decoy) => decoy.playerId === playerId) || null;
}

function receiptMatchesEntity(receipt, entity) {
  return entity != null && receipt.source === entity.playerId &&
    receipt.sourceSessionId === entity.sessionId && receipt.sourceLifeSeq === entity.lifeSeq &&
    receipt.serverSessionId === entity.serverSessionId && receipt.arenaStreamId === entity.arenaStreamId &&
    receipt.shotGroupId === entity.shotGroupId && receipt.sourceServerTick === entity.serverTick;
}

async function captureHeldRepelReceipts(observer, forced, actorId) {
  // Read at most one copy per immutable source/kind from the existing bounded
  // packet ring. No full-page state transfer or game-state hook is involved.
  const rows = await observer.evaluate(`(() => {
    const held = new Set(${JSON.stringify(forced.held_event_ids)});
    const selected = new Map();
    let repel = null;
    for (const entry of [...(window.__zubspacePacketLogFirst || []), ...(window.__zubspacePacketLog || [])]) {
      const b = entry.bytes;
      if (entry.direction !== 'received' || entry.bytesTruncated || !b || b.length < 74 || b[0] !== 125 || b[1] !== 2) continue;
      const id = (b[19] | b[20] << 8 | b[21] << 16 | b[22] << 24) >>> 0;
      const row = { seq: entry.seq, at: entry.at, channel: entry.detail?.channel, bytes: b };
      if (id === ${forced.repel_event_id} && b[2] === 1 && (b[72] & 31) === 5) {
        if (!repel || row.seq < repel.seq) repel = row;
      } else if (held.has(id) && (b[2] === 1 || b[2] === 4) && [6,8].includes(b[72] & 31)) {
        selected.set(b[2] + ':' + (b[72] & 31) + ':' + b.slice(27, 49).join(','), row);
      }
    }
    return [repel, ...[...selected.values()].slice(-32)].filter(Boolean);
  })()`);
  const receipts = rows.map(row => ({ ...row, fact: parseRealtimeWeaponReceipt(Buffer.from(row.bytes)) }))
    .filter(row => row.fact != null);
  const repel = receipts.find(row => row.fact.kind === 1 && row.fact.eventId === forced.repel_event_id);
  if (!repel) return null;
  const held = receipts.filter(row => row.seq > repel.seq && row.fact.source === actorId &&
    row.fact.serverSessionId === repel.fact.serverSessionId && row.fact.arenaStreamId === repel.fact.arenaStreamId &&
    forced.held_event_ids.includes(row.fact.eventId) && realtimeEventPrecedes(row.fact.eventId, forced.repel_event_id));
  if (![6,8].every(weapon => [1,4].every(kind => held.some(row => row.fact.kind === kind &&
    (row.fact.weapon & 31) === weapon && (kind !== 4 || forced.held_recovery_event_ids.includes(row.fact.eventId)))))) return null;
  return { repel, held };
}

function velocityChanged(before, after) {
  if (!before || !after) return false;
  return Math.hypot(
    after.velocityX - before.velocityX,
    after.velocityY - before.velocityY,
  ) > 100;
}

function exactAuthoritativeRepelState(entity, serverState) {
  const state = entity?.authoritativeRepel;
  if (!state || !serverState) return false;
  const close = (left, right) =>
    Number.isFinite(left) && Number.isFinite(right) && Math.abs(left - right) <= 0.001;
  const liveVelocityMatches =
    close(entity.velocityX, serverState.velocity_x) &&
    close(entity.velocityY, serverState.velocity_y);
  const velocityLengthSquared =
    serverState.velocity_x ** 2 + serverState.velocity_y ** 2;
  const offsetX = entity.x - serverState.position_x;
  const offsetY = entity.y - serverState.position_y;
  const elapsed = velocityLengthSquared > 0.000001
    ? (offsetX * serverState.velocity_x + offsetY * serverState.velocity_y) /
      velocityLengthSquared
    : 0;
  const livePositionMatches =
    Number.isFinite(entity.x) &&
    Number.isFinite(entity.y) &&
    Number.isFinite(elapsed) &&
    elapsed >= -0.002 &&
    Math.abs(
      entity.x - (serverState.position_x + serverState.velocity_x * elapsed),
    ) <= 0.05 &&
    Math.abs(
      entity.y - (serverState.position_y + serverState.velocity_y * elapsed),
    ) <= 0.05;
  return (
    state.sourcePlayerId === serverState.source_player_id &&
    String(state.sourceSessionId) === String(serverState.source_session_id) &&
    state.sourceLifeSeq === serverState.source_life_seq &&
    state.sourceServerTick === serverState.source_server_tick &&
    state.sourceWeaponData === serverState.source_weapon_data &&
    state.sourceProjectileIndex === serverState.source_projectile_index &&
    state.resultWeaponData === serverState.result_weapon_data &&
    state.expiresAtTick === serverState.expires_at_tick &&
    state.bouncesRemaining === serverState.bounces_remaining &&
    close(state.positionX, serverState.position_x) &&
    close(state.positionY, serverState.position_y) &&
    close(state.velocityX, serverState.velocity_x) &&
    close(state.velocityY, serverState.velocity_y) &&
    liveVelocityMatches &&
    livePositionMatches
  );
}

function realtimeEventPrecedes(earlier, later) {
  if (!Number.isInteger(earlier) || !Number.isInteger(later)) return false;
  const distance = (later - earlier) >>> 0;
  return distance !== 0 && distance < 0x80000000;
}

function infoProjectileSpawns(logs) {
  const text = Array.isArray(logs) ? logs.join("") : String(logs || "");
  const fields = [
    "projectile_id",
    "shot_group_id",
    "owner_id",
    "owner_life_seq",
    "server_tick",
    "weapon",
    "x",
    "y",
    "velocity_x",
    "velocity_y",
    "bounces_remaining",
  ];
  return text.split(/\n/).flatMap((line, logIndex) => {
    if (!line.includes("server projectile spawned")) return [];
    const values = Object.fromEntries(fields.map((field) => {
      const match = line.match(new RegExp(`(?:^|\\s)${field}=(-?\\d+(?:\\.\\d+)?)`));
      return [field, match == null ? null : Number(match[1])];
    }));
    if (Object.values(values).some((value) => !Number.isFinite(value))) return [];
    const expiry = line.match(/(?:^|\s)expiry_tick=(?:Some\()?([0-9]+)\)?/);
    return [{
      type: "server_projectile_spawned",
      projectileId: values.projectile_id,
      shotGroupId: values.shot_group_id,
      ownerId: values.owner_id,
      ownerLifeSeq: values.owner_life_seq,
      serverTick: values.server_tick,
      weapon: values.weapon,
      x: values.x,
      y: values.y,
      velocityX: values.velocity_x,
      velocityY: values.velocity_y,
      expiryTick: expiry == null ? null : Number(expiry[1]),
      bouncesRemaining: values.bounces_remaining,
      logIndex,
    }];
  });
}

function serverProjectileEvents(server) {
  const structured = parseServerProjectileEvents(server.logs);
  return structured.length > 0 ? structured : infoProjectileSpawns(server.logs);
}

async function runExtremeZeroInventoryScenario({ actor, peer, server, actorId, peerId }) {
  const [actorBefore, peerBefore] = await Promise.all([
    actor.pageState(),
    peer.pageState(),
  ]);
  const actorLocalBefore = localState(actorBefore);
  const peerLocalBefore = localState(peerBefore);
  for (const [label, state] of [["actor", actorBefore], ["peer", peerBefore]]) {
    if (state.uiMetrics?.playerShip !== 0 || state.uiMetrics?.selfRosterShip !== 0) {
      throw new Error(`Extreme authored-zero ${label} requires an exact Warbird identity`);
    }
  }
  const expectedZero = {
    decoys: actorLocalBefore.decoys,
    thors: actorLocalBefore.thors,
    bursts: actorLocalBefore.bursts,
    rockets: actorLocalBefore.rockets,
    repels: peerLocalBefore.repels,
  };
  if (Object.values(expectedZero).some((value) => value !== 0)) {
    throw new Error(`Extreme Warbird authored zero-inventory baseline changed: ${JSON.stringify(expectedZero)}`);
  }
  const actorCounts = actorBefore.uiMetrics.semanticInputCounts || {};
  const peerCounts = peerBefore.uiMetrics.semanticInputCounts || {};
  const projectileCount = serverProjectileEvents(server)
    .filter((event) => event.type === "server_projectile_spawned").length;
  const rocketCount = parseZubspaceEvents(server.logs)
    .filter(({ event }) => event.type === "server_rocket_activated").length;
  const mutationCount = parseZubspaceEvents(server.logs)
    .filter(({ event }) => event.type === "server_repel_projectiles_applied").length;

  await actor.press("F5", 70);
  await actor.press("F6", 70);
  await actor.chord("Shift", "Delete", 70);
  await actor.press("F3", 70);
  await peer.press("q", 80);
  await waitUntil("Extreme zero-inventory real-key semantic edges settle", async () => {
    const [actorState, peerState] = await Promise.all([
      actor.metricsState(),
      peer.metricsState(),
    ]);
    const actorMetrics = actorState.uiMetrics || {};
    const peerMetrics = peerState.uiMetrics || {};
    return (
      semanticDelta(actorCounts, actorMetrics.semanticInputCounts, "decoy") === 1 &&
      semanticDelta(actorCounts, actorMetrics.semanticInputCounts, "thor") === 1 &&
      semanticDelta(actorCounts, actorMetrics.semanticInputCounts, "burst") === 1 &&
      semanticDelta(actorCounts, actorMetrics.semanticInputCounts, "rocket") === 1 &&
      semanticDelta(peerCounts, peerMetrics.semanticInputCounts, "repel") === 1
    );
  }, 12_000);
  await sleep(500);

  const [actorAfter, peerAfter] = await Promise.all([
    actor.pageState(),
    peer.pageState(),
  ]);
  const actorLocalAfter = localState(actorAfter);
  const peerLocalAfter = localState(peerAfter);
  const unchanged = {
    decoys: actorLocalAfter.decoys,
    thors: actorLocalAfter.thors,
    bursts: actorLocalAfter.bursts,
    rockets: actorLocalAfter.rockets,
    repels: peerLocalAfter.repels,
  };
  const projectilesAfter = serverProjectileEvents(server)
    .filter((event) => event.type === "server_projectile_spawned").length;
  const rocketsAfter = parseZubspaceEvents(server.logs)
    .filter(({ event }) => event.type === "server_rocket_activated").length;
  const mutationsAfter = parseZubspaceEvents(server.logs)
    .filter(({ event }) => event.type === "server_repel_projectiles_applied").length;
  if (
    Object.values(unchanged).some((value) => value !== 0) ||
    actorLocalAfter.decoysActive !== 0 ||
    actorLocalAfter.thorsActive !== 0 ||
    actorLocalAfter.bullets !== 0 ||
    actorAfter.uiMetrics.playerRocketTimer !== 0 ||
    projectilesAfter !== projectileCount ||
    rocketsAfter !== rocketCount ||
    mutationsAfter !== mutationCount ||
    actorAfter.uiMetrics.playerLifeSeq !== actorBefore.uiMetrics.playerLifeSeq ||
    peerAfter.uiMetrics.playerLifeSeq !== peerBefore.uiMetrics.playerLifeSeq
  ) {
    throw new Error(`Extreme zero-inventory special action escaped its authored gate: ${JSON.stringify({
      expectedZero,
      unchanged,
      actorBefore: actorBefore.uiMetrics,
      actorAfter: actorAfter.uiMetrics,
      peerBefore: peerBefore.uiMetrics,
      peerAfter: peerAfter.uiMetrics,
      projectileCount,
      projectilesAfter,
      rocketCount,
      rocketsAfter,
      mutationCount,
      mutationsAfter,
    })}`);
  }
  return {
    sourceHonestZeroInventory: true,
    authoredInitialBounty: 3,
    controls: firefoxActor
      ? ["Firefox F5", "Firefox F6", "Firefox Shift+Delete", "Firefox F3", "Chrome q"]
      : ["Chrome F5", "Chrome F6", "Chrome Shift+Delete", "Chrome F3", "Chrome q"],
    actorId,
    peerId,
    inventory: unchanged,
    semanticDeltas: {
      decoy: semanticDelta(actorCounts, actorAfter.uiMetrics.semanticInputCounts, "decoy"),
      thor: semanticDelta(actorCounts, actorAfter.uiMetrics.semanticInputCounts, "thor"),
      burst: semanticDelta(actorCounts, actorAfter.uiMetrics.semanticInputCounts, "burst"),
      rocket: semanticDelta(actorCounts, actorAfter.uiMetrics.semanticInputCounts, "rocket"),
      repel: semanticDelta(peerCounts, peerAfter.uiMetrics.semanticInputCounts, "repel"),
    },
    authoritativeMutations: {
      projectiles: projectilesAfter - projectileCount,
      rocketActivations: rocketsAfter - rocketCount,
      repelMutations: mutationsAfter - mutationCount,
    },
    final: {
      actor: compactMetrics(actorAfter.uiMetrics),
      peer: compactMetrics(peerAfter.uiMetrics),
    },
  };
}

async function runSpecialWeaponScenario({
  actor,
  peer,
  observer,
  server,
  actorId,
  peerId,
  observerId,
}) {
  if (arena === "extreme") {
    return runExtremeZeroInventoryScenario({ actor, peer, server, actorId, peerId });
  }
  const initial = await Promise.all([actor.pageState(), peer.pageState()]);
  const actorInitial = initial[0];
  const peerInitial = initial[1];
  const actorLifeSeq = actorInitial.uiMetrics?.playerLifeSeq;
  const peerLifeSeqBeforeSetup = peerInitial.uiMetrics?.playerLifeSeq;
  const initialActorLocal = localState(actorInitial);
  const initialPeerLocal = localState(peerInitial);
  for (const [field, value] of Object.entries({
    decoys: initialActorLocal.decoys,
    thors: initialActorLocal.thors,
    bursts: initialActorLocal.bursts,
    rockets: initialActorLocal.rockets,
    repels: initialPeerLocal.repels,
  })) {
    if (!Number.isInteger(value) || value < 1) {
      throw new Error(`${arena} maximum entry has no ${field} for the replay: ${value}`);
    }
  }
  if (initialActorLocal.decoys < 2) {
    throw new Error(
      `${arena} maximum entry needs two Decoys for the wall-activation/Repel replay: ${initialActorLocal.decoys}`,
    );
  }

  // Product FFA frequencies are immutable. Physically separate the passive
  // third observer instead of putting it on the shooter's team to avoid hits.
  const targetFrequency = peerInitial.uiMetrics.playerFrequency;
  if (targetFrequency === actorInitial.uiMetrics.playerFrequency) throw new Error("FFA peers share a frequency");
  const observerSettlement = await settleSpecialWeaponLane(observer, {
    x: peerInitial.uiMetrics.playerX + 64,
    y: peerInitial.uiMetrics.playerY + 224,
  }, { routeTimeoutMs: 15_000, maximumDesiredSpeed: 35, desiredSpeedScale: 0.3, progressTimeoutMs: 5_000 });
  if (!observerSettlement.reached) throw new Error(`FFA observer did not settle clear of the firing lane: ${JSON.stringify(observerSettlement)}`);
  await Promise.all([
    waitForPlayersReady(actor, peer, server),
    waitForPlayersReady(actor, observer, server),
  ]);
  const peerLifeSeq = (await peer.metricsState()).uiMetrics?.playerLifeSeq;
  const observerLifeSeq = (await observer.metricsState()).uiMetrics?.playerLifeSeq;
  if (peerLifeSeq !== peerLifeSeqBeforeSetup) throw new Error("FFA staging unexpectedly reset the peer life");
  await waitForWeaponReady(actor, "special actor initial");
  await waitForWeaponReady(peer, "special peer initial");
  await waitForWeaponReady(observer, "special observer initial");

  // Separate the hostile pair far enough that Thor cannot overlap the peer on
  // the spawn tick, then stop the shooter through ordinary keyboard feedback.
  // Keep the peer stationary: Burst projectile 1 activates on authored door
  // tile 169 north of spawn and returns on a pixel-truncated vertical line, so
  // moving the target would turn contact into a race against frictionless
  // drift. The firing leg below also proves a consecutive closed-door phase.
  // Both ships, the live Decoy, and Thor remain inside Repel range.
  const peerStage = (await peer.metricsState()).uiMetrics || {};
  const stageTarget = {
    x: peerStage.playerX,
    y: peerStage.playerY + 112,
  };
  // Own position and velocity in one closed-loop operation. The former
  // arrive -> brake -> chase sequence could stop just outside the narrow
  // Burst lane, then re-accelerate through a valid near-target sample before
  // its independent refinement poll observed it.
  const stageSettlement = await settleSpecialWeaponLane(
    actor,
    stageTarget,
    {
      routeTimeoutMs: 15_000,
      maximumDesiredSpeed: 35,
      desiredSpeedScale: 0.3,
      progressTimeoutMs: 5_000,
    },
  );
  if (!stageSettlement.reached) {
    throw new Error(`special-weapon hostile staging did not settle: ${JSON.stringify({
      stageTarget,
      stageSettlement,
    })}`);
  }
  await sleep(100);
  const staged = await Promise.all([actor.pageState(), peer.pageState()]);
  const stagedDistance = Math.hypot(
    staged[0].uiMetrics.playerX - staged[1].uiMetrics.playerX,
    staged[0].uiMetrics.playerY - staged[1].uiMetrics.playerY,
  );
  const stagedLateralError = Math.abs(staged[0].uiMetrics.playerX - staged[1].uiMetrics.playerX);
  if (!(stagedDistance > 72 && stagedDistance < 128) || stagedLateralError > 8) {
    throw new Error(`special-weapon hostile staging left the Burst/Repel lane: ${JSON.stringify({
      stagedDistance,
      stagedLateralError,
      stageTarget,
      stageSettlement,
    })}`);
  }

  const decoyBefore = await actor.metricsState();
  const decoyCountsBefore = decoyBefore.uiMetrics.semanticInputCounts || {};
  // A complete failure-artifact read is intentionally heavyweight enough to
  // exercise presentation catch-up. Re-establish the exact drawn-view
  // boundary after collecting the baseline so this genuine one-shot proves
  // Decoy authority rather than an expected held-view rejection.
  await armWeaponInput(actor, "actor immediately before Decoy");
  await pressArmed(actor, "F5", 70);
  let decoyPresentation = null;
  await waitUntil("real F5 Decoy authority and ordinary presentation", async () => {
    const [actorState, peerState, observerState] = await Promise.all([
      actor.metricsState(),
      peer.metricsState(),
      observer.metricsState(),
    ]);
    const actorMetrics = actorState.uiMetrics || {};
    const peerMetrics = peerState.uiMetrics || {};
    const actorLocal = actorMetrics.localGameplayState || {};
    const peerLocal = peerMetrics.localGameplayState || {};
    const remotePresentationReady = forcedRealtimeRepelReorder || (
      peerLocal.decoysActive >= 1 &&
      peerLocal.decoyOrdinaryShipPresentationCount >= 1 &&
      peerLocal.decoyNameLabelPresentationCount >= 1 &&
      peerLocal.decoyRadarPresentationCount >= 1 &&
      remoteDecoy(observerState.uiMetrics, actorId) != null
    );
    const converged = (
      semanticDelta(decoyCountsBefore, actorMetrics.semanticInputCounts, "decoy") === 1 &&
      actorLocal.decoys === initialActorLocal.decoys - 1 &&
      actorLocal.decoysActive >= 1 &&
      actorLocal.bombCooldown > 0 &&
      remotePresentationReady
    );
    if (converged) decoyPresentation = peerMetrics;
    return converged;
  }, 12_000);

  await waitForWeaponReady(actor, "actor after Decoy");
  const burstStageSettlement = await settleSpecialWeaponLane(
    actor,
    stageTarget,
    {
      routeTimeoutMs: 8_000,
      maximumDesiredSpeed: 18,
      desiredSpeedScale: 0.25,
      progressTimeoutMs: 5_000,
    },
  );
  if (!burstStageSettlement.reached) {
    throw new Error(`Burst shooter did not retain its static-wall lane: ${JSON.stringify({
      stageTarget,
      burstStageSettlement,
    })}`);
  }
  const projectileCountBeforeBurst = serverProjectileEvents(server)
    .filter((event) => event.type === "server_projectile_spawned").length;
  const damageEventsBeforeBurst = parseZubspaceEvents(server.logs)
    .filter(({ event }) => event.type === "server_damage_applied").length;
  let burstDoorWindow = null;
  let burstActorStage = null;
  let burstPeerStage = null;
  let burstStageDistance = null;
  let burstStageLateralError = null;
  let burstCountsBefore = null;
  let burstLaneRestagings = 0;
  let lastBurstLaneRestaging = null;
  await actor.focus();
  await waitUntil("authored Burst door 169 remains closed across the contact path", async () => {
    // Read the peer first and the firing actor last. In Firefox a successful
    // actor readiness sample followed by a slower command to another browser
    // can be stale before WebDriver delivers Delete. The peer is stationary,
    // so its earlier position remains the truthful lane anchor while the
    // final actor read owns the press-time door/view boundary.
    const peerState = await peer.metricsState();
    const actorState = await actor.metricsState();
    const actorMetrics = actorState.uiMetrics || {};
    const peerMetrics = peerState.uiMetrics || {};
    const distance = Math.hypot(
      actorMetrics.playerX - peerMetrics.playerX,
      actorMetrics.playerY - peerMetrics.playerY,
    );
    const lateralError = Math.abs(actorMetrics.playerX - peerMetrics.playerX);
    if (!(distance > 72 && distance < 180) || lateralError > 8) {
      // Subspace ships retain velocity. The real-key settling controller can
      // finish below one physical thrust quantum yet still drift out of this
      // deliberately narrow one-tile Burst lane while waiting for the seeded
      // door phase. Reacquire the same authored target with ordinary keyboard
      // input; do not widen the collision corridor or bypass the door check.
      burstLaneRestagings += 1;
      lastBurstLaneRestaging = await settleSpecialWeaponLane(
        actor,
        stageTarget,
        {
          routeTimeoutMs: 4_000,
          maximumDesiredSpeed: 18,
          desiredSpeedScale: 0.25,
          progressTimeoutMs: 2_000,
        },
      );
      if (!lastBurstLaneRestaging.reached) {
        throw new Error(`Burst lane restaging failed: ${JSON.stringify({
          distance,
          lateralError,
          lastBurstLaneRestaging,
        })}`);
      }
      return false;
    }
    if (!authoredDoorClosedForCurrentAndNext(actorMetrics, 169)) return false;
    if (actorMetrics.combatViewTimeline?.weaponReady !== true) return false;
    const local = actorMetrics.localGameplayState || {};
    burstDoorWindow = {
      doorSeed: local.doorSeed,
      doorSeedNext: local.doorSeedNext,
      doorTimer: local.doorTimer,
      doorClockFresh: local.doorClockFresh,
    };
    burstActorStage = actorState;
    burstPeerStage = peerState;
    burstStageDistance = distance;
    burstStageLateralError = lateralError;
    burstCountsBefore = actorMetrics.semanticInputCounts || {};
    return true;
  }, 30_000);
  // Fire immediately from the minimal phase/lane sample. A full page-state
  // capture contains multi-megabyte retained traces and can outlive the
  // authored 1.2-second door phase, changing the physical fixture before the
  // genuine keyboard chord reaches the server.
  await chordArmed(actor, "Shift", "Delete", 70);
  let burstSpawns = [];
  let inactiveBurstPresentation = null;
  await waitUntil("real Shift+Delete Burst reaches server and peer presentation", async () => {
    burstSpawns = newProjectileSpawns(
      server,
      projectileCountBeforeBurst,
      actorId,
      7,
    );
    const [actorState, peerState] = await Promise.all([
      actor.metricsState(),
      peer.metricsState(),
    ]);
    const actorMetrics = actorState.uiMetrics || {};
    const actorLocal = actorMetrics.localGameplayState || {};
    const remoteBursts = (peerState.uiMetrics?.remoteProjectileSources || []).filter((source) =>
      source.playerId === actorId && source.kind === "burst"
    );
    inactiveBurstPresentation ||= remoteBursts.find((source) => source.active === false) || null;
    return (
      semanticDelta(burstCountsBefore, actorMetrics.semanticInputCounts, "burst") === 1 &&
      actorLocal.bursts === initialActorLocal.bursts - 1 &&
      actorLocal.burstCooldown > 0 &&
      actorLocal.bombCooldown > 0 &&
      burstSpawns.length >= 1 &&
      inactiveBurstPresentation != null
    );
  }, 12_000);
  let activeBurstPresentation = null;
  let burstDamageEvent = null;
  let burstBrowserDamage = null;
  await waitUntil(
    forcedRealtimeRepelReorder
      ? "Burst first-wall activation reaches the peer before forced Repel reorder"
      : "Burst first-wall activation reaches the peer and deals authored damage",
    async () => {
      const peerState = await peer.pageState();
      activeBurstPresentation = (peerState.uiMetrics?.remoteProjectileSources || []).find(
        (source) => source.playerId === actorId && source.kind === "burst" && source.active === true,
      ) || null;
      burstDamageEvent = parseZubspaceEvents(server.logs)
        .filter(({ event }) => event.type === "server_damage_applied")
        .slice(damageEventsBeforeBurst)
        .map(({ event }) => event)
        .find((event) =>
          event.shooter_id === actorId &&
          event.victim_id === peerId &&
          (event.weapon & 0x1f) === 7 &&
          event.damage === 700 &&
          event.energy_before - event.energy_after === 700
        ) || null;
      burstBrowserDamage = (peerState.damageTrace || []).find((event) =>
        event.attackerId === actorId &&
        event.victimId === peerId &&
        (event.weapon & 0x1f) === 7 &&
        event.damage === 700 &&
        event.energyBefore - event.energyAfter === 700
      ) || null;
      return (
        activeBurstPresentation != null &&
        (
          forcedRealtimeRepelReorder ||
          (burstDamageEvent != null && burstBrowserDamage != null)
        )
      );
    },
    8_000,
  );

  // Keep Burst's narrow door/contact geometry above. Thor's reference sensor
  // is wider: Chaos raw-L0 has (ProximityDistance3 +3)*18 =108px reach against
  // a14px Warbird. The old106px settled separation armed it before Q. Stage
  // both hostile observers outside that sensor, still inside Repel's square.
  const thorRepelOrigin = (await peer.metricsState()).uiMetrics;
  const thorRepelStage = {
    actor: { x: thorRepelOrigin.playerX, y: thorRepelOrigin.playerY + 160 },
    observer: { x: thorRepelOrigin.playerX + 224, y: thorRepelOrigin.playerY + 224 },
  };
  const thorRepelStaging = {};
  for (const [label, client] of [["observer", observer], ["actor", actor]]) {
    thorRepelStaging[label] = await settleSpecialWeaponLane(client, thorRepelStage[label], {
      routeTimeoutMs: 15_000, maximumDesiredSpeed: 35, desiredSpeedScale: 0.3, progressTimeoutMs: 5_000,
    });
    if (!thorRepelStaging[label].reached) throw new Error(`Thor/Repel ${label} staging failed`);
  }

  // The Burst wall/damage leg can outlive the first Decoy's authored lifetime,
  // especially in headed Firefox. Create a fresh, real-key Decoy immediately
  // before Thor+Repel so this leg proves one coherent simultaneous mutation
  // rather than depending on a presentation entity that has correctly expired.
  const decoyRefreshBefore = await actor.metricsState();
  const decoyRefreshCounts = decoyRefreshBefore.uiMetrics.semanticInputCounts || {};
  await armWeaponInput(actor, "actor immediately before Repel Decoy refresh");
  await pressArmed(actor, "F5", 70);
  await waitUntil("fresh Decoy reaches third observer before Repel", async () => {
    const [actorState, observerState] = await Promise.all([
      actor.metricsState(),
      observer.metricsState(),
    ]);
    const actorMetrics = actorState.uiMetrics || {};
    const actorLocal = actorMetrics.localGameplayState || {};
    return (
      semanticDelta(decoyRefreshCounts, actorMetrics.semanticInputCounts, "decoy") === 1 &&
      actorLocal.decoys === initialActorLocal.decoys - 2 &&
      actorLocal.decoysActive >= 1 &&
      (forcedRealtimeRepelReorder || remoteDecoy(observerState.uiMetrics, actorId) != null)
    );
  }, 12_000);

  await waitForWeaponReady(actor, "actor after refreshed Decoy");
  const [thorBefore, thorPeerBefore] = await Promise.all([
    actor.metricsState(),
    peer.metricsState(),
  ]);
  const thorCountsBefore = thorBefore.uiMetrics.semanticInputCounts || {};
  // Capture every Repel baseline before Thor exists. Once the server accepts
  // F6, the genuine Q must follow immediately: a Thor can leave owner safety
  // and return into the pixel-truncated explosive contact box in about ten
  // server ticks, so post-spawn page/metrics reads would change the fixture.
  const [repelObserverBefore, repelBefore, repelTargetBefore] = await Promise.all([
    observer.metricsState(),
    peer.metricsState(),
    actor.metricsState(),
  ]);
  const observerThorBefore = remoteProjectile(repelObserverBefore.uiMetrics, actorId, "thor");
  const observerDecoyBefore = remoteDecoy(repelObserverBefore.uiMetrics, actorId);
  if (
    forcedRealtimeRepelReorder
      ? observerThorBefore != null || observerDecoyBefore != null
      : !observerDecoyBefore
  ) {
    throw new Error(`third observer pre-Repel reorder state mismatched: ${JSON.stringify({
      forcedRealtimeRepelReorder,
      observerThorBefore,
      observerDecoyBefore,
      metrics: repelObserverBefore.uiMetrics,
    })}`);
  }
  const repelCountsBefore = repelBefore.uiMetrics.semanticInputCounts || {};
  const repelEventsBefore = parseZubspaceEvents(server.logs)
    .filter(({ event }) => event.type === "server_repel_projectiles_applied").length;
  const projectileCountBeforeThor = serverProjectileEvents(server)
    .filter((event) => event.type === "server_projectile_spawned").length;
  // Turn Thor across the staged line so it cannot contact the nearby hostile
  // ship before the immediate Q mutation below.
  await actor.press("ArrowRight", 400);
  const thorLaunchGeometry = (await Promise.all([actor, peer, observer].map(client => client.metricsState())))
    .map(state => state.uiMetrics);
  const [launchActor, ...launchTargets] = thorLaunchGeometry;
  thorRepelStaging.clearance = launchTargets.map(target => Math.max(
    Math.abs(launchActor.playerX - target.playerX), Math.abs(launchActor.playerY - target.playerY)));
  if (!thorRepelStaging.clearance.every(distance => distance > 128 && distance < 400))
    throw new Error(`Thor starts inside a hostile sensor or outside the Repel setup: ${JSON.stringify(thorRepelStaging)}`);
  // Arm both genuine one-shot boundaries before launching Thor. Waiting for
  // peer readiness after the spawn can consume its entire 400 px Repel
  // envelope even though both actions are individually authoritative.
  await Promise.all([
    armWeaponInput(actor, "actor immediately before Thor"),
    armWeaponInput(peer, "peer immediately before Thor/Repel"),
  ]);
  await pressArmed(actor, "F6", 70);
  let thorSpawns = [];
  await waitUntil("real F6 Thor reaches server authority", async () => {
    thorSpawns = newProjectileSpawns(
      server,
      projectileCountBeforeThor,
      actorId,
      8,
    );
    // Q must follow the authoritative spawn while Thor remains inside the
    // authored 400 px square. Waiting for Firefox's observer presentation can
    // consume more than two seconds, by which time the 200 px/s projectile is
    // already outside the legal Repel range. The exact public mutation below
    // still proves eventual cross-channel convergence.
    return thorSpawns.length >= 1;
  }, 12_000);
  await pressArmed(peer, "q", 80);
  let repelActivation = null;
  await waitUntil("real Q spends Repel inventory and starts shared cooldowns", async () => {
    const peerMetrics = (await peer.metricsState()).uiMetrics || {};
    const peerLocal = peerMetrics.localGameplayState || {};
    const converged = (
      semanticDelta(repelCountsBefore, peerMetrics.semanticInputCounts, "repel") === 1 &&
      peerLocal.repels === initialPeerLocal.repels - 1 &&
      peerLocal.repelCooldown > 0 &&
      peerLocal.bombCooldown > 0
    );
    if (converged) {
      repelActivation = {
        inventory: peerLocal.repels,
        repelCooldown: peerLocal.repelCooldown,
        bombCooldown: peerLocal.bombCooldown,
      };
    }
    return converged;
  }, 8_000);
  let repelMutation = null;
  let authoritativeRepelStates = null;
  let forcedReorderEvidence = null;
  let forcedReorderReceipts = null;
  await waitUntil("real Q mutates server projectile, server Decoy, and hostile player", async () => {
    repelMutation = parseZubspaceEvents(server.logs)
      .filter(({ event }) => event.type === "server_repel_projectiles_applied")
      .slice(repelEventsBefore)
      .map(({ event }) => event)
      .find((event) =>
        event.source_player_id === peerId &&
        event.projectiles_repelled >= 1 &&
        event.decoys_repelled >= 1
      ) || null;
    const serverEntityStates = parseZubspaceEvents(server.logs)
      .map(({ event }) => event)
      .filter((event) =>
        event.type === "server_repel_entity_state" &&
        event.source_player_id === actorId
      );
    const thorStates = serverEntityStates.filter((event) =>
      event.entity_kind === "projectile" &&
      (event.source_weapon_data & 0x1f) === 8
    );
    const decoyStates = serverEntityStates.filter((event) =>
      event.entity_kind === "decoy" &&
      (event.source_weapon_data & 0x1f) === 6
    );
    forcedReorderEvidence = forcedRealtimeRepelReorder
      ? parseZubspaceEvents(server.logs)
          .map(({ event }) => event)
          .find((event) =>
            event.type === "server_realtime_repel_reorder_forced" &&
            event.recipient_id === observerId &&
            Array.isArray(event.held_weapon_types) &&
            event.held_weapon_types.includes(6) &&
            event.held_weapon_types.includes(8) &&
            Array.isArray(event.held_recovery_event_ids) &&
            event.held_recovery_event_ids.length >= 1 &&
            event.held_recovery_event_ids.every((id) => event.held_event_ids?.includes(id) && realtimeEventPrecedes(id, event.repel_event_id)) &&
            Array.isArray(event.held_event_ids) &&
            event.held_event_ids.every((eventId) =>
              realtimeEventPrecedes(eventId, event.repel_event_id)
            )
          ) || null
      : null;
    if (forcedReorderEvidence && !forcedReorderReceipts) {
      forcedReorderReceipts = await captureHeldRepelReceipts(observer, forcedReorderEvidence, actorId);
    }
    const [peerState, actorState, observerState] = await Promise.all([
      peer.metricsState(),
      actor.metricsState(),
      observer.metricsState(),
    ]);
    const peerMetrics = peerState.uiMetrics || {};
    const actorMetrics = actorState.uiMetrics || {};
    const peerLocal = peerMetrics.localGameplayState || {};
    const actorLocal = actorMetrics.localGameplayState || {};
    const observerMetrics = observerState.uiMetrics || {};
    const observerThorAfter = (observerMetrics.remoteProjectileSources || []).find((entity) =>
      entity.playerId === actorId &&
      entity.kind === "thor" &&
      thorStates.some((state) => exactAuthoritativeRepelState(entity, state))
    ) || null;
    const observerDecoysAfter = (observerMetrics.remoteDecoys || []).filter((entity) =>
      entity.playerId === actorId
    );
    const observerDecoyAfter = observerDecoyBefore == null
      ? observerDecoysAfter.find((entity) =>
          decoyStates.some((state) => exactAuthoritativeRepelState(entity, state))
        ) || null
      : observerDecoysAfter.find((entity) => sameDecoyEntity(entity, observerDecoyBefore)) || null;
    const thorState = thorStates.find((state) =>
      exactAuthoritativeRepelState(observerThorAfter, state)
    ) || null;
    const decoyState = decoyStates.find((state) =>
      exactAuthoritativeRepelState(observerDecoyAfter, state)
    ) || null;
    const peerDecoyAfter = (peerMetrics.remoteDecoys || []).find((entity) =>
      entity.playerId === actorId && exactAuthoritativeRepelState(entity, decoyState)
    ) || null;
    authoritativeRepelStates = thorState && decoyState
      ? { thor: thorState, decoy: decoyState }
      : null;
    const forcedObserverConverged = (
      forcedReorderEvidence != null &&
      observerThorAfter != null &&
      observerDecoyAfter != null &&
      forcedReorderReceipts != null &&
      [observerThorAfter, observerDecoyAfter].every(entity => [1,4].every(kind =>
        forcedReorderReceipts.held.some(row => row.fact.kind === kind && receiptMatchesEntity(row.fact, entity)))) &&
      exactAuthoritativeRepelState(observerThorAfter, thorState) &&
      exactAuthoritativeRepelState(observerDecoyAfter, decoyState) &&
      peerDecoyAfter != null &&
      exactAuthoritativeRepelState(peerDecoyAfter, decoyState)
    );
    const ordinaryObserverConverged = (
      (observerThorBefore == null || velocityChanged(observerThorBefore, observerThorAfter)) &&
      velocityChanged(observerDecoyBefore, observerDecoyAfter) &&
      exactAuthoritativeRepelState(observerThorAfter, thorState) &&
      exactAuthoritativeRepelState(observerDecoyAfter, decoyState) &&
      exactAuthoritativeRepelState(peerDecoyAfter, decoyState)
    );
    const decoyPresentationConverged = (
      peerLocal.decoysActive >= 1 &&
      peerLocal.decoyOrdinaryShipPresentationCount >= 1 &&
      peerLocal.decoyNameLabelPresentationCount >= 1 &&
      peerLocal.decoyRadarPresentationCount >= 1
    );
    if (decoyPresentationConverged) decoyPresentation = peerMetrics;
    return (
      repelMutation != null &&
      authoritativeRepelStates != null &&
      semanticDelta(thorCountsBefore, actorMetrics.semanticInputCounts, "thor") === 1 &&
      actorLocal.thors === initialActorLocal.thors - 1 &&
      semanticDelta(repelCountsBefore, peerMetrics.semanticInputCounts, "repel") === 1 &&
      peerLocal.repels === initialPeerLocal.repels - 1 &&
      (peerMetrics.remoteProjectileSpawns ?? 0) >
        (thorPeerBefore.uiMetrics.remoteProjectileSpawns ?? 0) &&
      (actorMetrics.remoteRepelSpawns ?? 0) > (repelTargetBefore.uiMetrics.remoteRepelSpawns ?? 0) &&
      (
        (actorLocal.repelDeliveryFramesApplied ?? 0) >
          (localState(repelTargetBefore).repelDeliveryFramesApplied ?? 0) ||
        actorLocal.repelTimer > 0
      ) &&
      (observerMetrics.remoteRepelSpawns ?? 0) ===
        (repelObserverBefore.uiMetrics.remoteRepelSpawns ?? 0) + 1 &&
      decoyPresentationConverged &&
      (
        forcedRealtimeRepelReorder
          ? forcedObserverConverged
          : ordinaryObserverConverged
      )
    );
  }, 12_000);

  await sleep(250);
  const repelObserverAfter = await observer.metricsState();
  const observerRepelDelta =
    (repelObserverAfter.uiMetrics.remoteRepelSpawns ?? 0) -
    (repelObserverBefore.uiMetrics.remoteRepelSpawns ?? 0);
  if (observerRepelDelta !== 1) {
    throw new Error(`third observer applied public Repel ${observerRepelDelta} times`);
  }

  const rocketBefore = await actor.metricsState();
  const rocketCountsBefore = rocketBefore.uiMetrics.semanticInputCounts || {};
  const rocketEventsBefore = parseZubspaceEvents(server.logs)
    .filter(({ event }) => event.type === "server_rocket_activated").length;
  await armWeaponInput(actor, "actor before Rocket regression");
  await pressArmed(actor, "F3", 70);
  let rocketEvent = null;
  await waitUntil("real F3 preserves authoritative Rocket activation", async () => {
    rocketEvent = parseZubspaceEvents(server.logs)
      .filter(({ event }) => event.type === "server_rocket_activated")
      .slice(rocketEventsBefore)
      .map(({ event }) => event)
      .find((event) => event.player_id === actorId) || null;
    const state = await actor.metricsState();
    const metrics = state.uiMetrics || {};
    return (
      rocketEvent != null &&
      semanticDelta(rocketCountsBefore, metrics.semanticInputCounts, "rocket") === 1 &&
      metrics.playerRockets === rocketBefore.uiMetrics.playerRockets - 1 &&
      metrics.playerRocketTimer > 0 &&
      metrics.pendingRocketActivationSequence == null &&
      metrics.lastSelfServerRocketActivationSequence != null
    );
  }, 12_000);

  const final = await Promise.all([
    actor.pageState(),
    peer.pageState(),
    observer.pageState(),
  ]);
  for (const [label, state, expectedLife] of [
    ["actor", final[0], actorLifeSeq],
    ["peer", final[1], peerLifeSeq],
    ["observer", final[2], observerLifeSeq],
  ]) {
    if (
      state.uiMetrics?.playerAlive !== true ||
      state.uiMetrics?.serverPlayerAlive !== true ||
      state.uiMetrics?.playerLifeSeq !== expectedLife
    ) {
      throw new Error(`${label} lifecycle changed during special-weapon replay: ${JSON.stringify(state.uiMetrics)}`);
    }
  }

  return {
    forcedRealtimeRepelReorder,
    forcedReorderEvidence,
    forcedReorderReceipts,
    controls: firefoxActor
      ? [
          "Firefox F5",
          "Firefox Shift+Delete",
          "Firefox F5",
          "Firefox F6",
          "Firefox F3",
          "Chrome q",
        ]
      : [
          "Chrome F5",
          "Chrome Shift+Delete",
          "Chrome F5",
          "Chrome F6",
          "Chrome F3",
          "Chrome q",
        ],
    actorId,
    peerId,
    observerId,
    actorLifeSeq,
    peerLifeSeq,
    observerLifeSeq,
    peerLifeSeqBeforeSetup,
    stagedDistance,
    initialInventory: {
      actor: {
        decoys: initialActorLocal.decoys,
        thors: initialActorLocal.thors,
        bursts: initialActorLocal.bursts,
        rockets: initialActorLocal.rockets,
      },
      peer: { repels: initialPeerLocal.repels },
    },
    decoyPresentation: {
      ordinaryShip: decoyPresentation.localGameplayState.decoyOrdinaryShipPresentationCount,
      nameLabel: decoyPresentation.localGameplayState.decoyNameLabelPresentationCount,
      radar: decoyPresentation.localGameplayState.decoyRadarPresentationCount,
      ownerFrequency: actorInitial.uiMetrics.playerFrequency,
      viewerFrequency: targetFrequency,
    },
    thorSpawns,
    repelActivation,
    repelMutation,
    authoritativeRepelStates,
    thirdObserverRepelPresentation: {
      remoteRepelDelta: observerRepelDelta,
      thorBefore: observerThorBefore,
      thorAfter: remoteProjectile(repelObserverAfter.uiMetrics, actorId, "thor"),
      decoyBefore: observerDecoyBefore,
      decoyAfter: remoteDecoy(repelObserverAfter.uiMetrics, actorId),
    },
    burstDamageStage: {
      distance: burstStageDistance,
      actor: {
        x: burstActorStage.uiMetrics.playerX,
        y: burstActorStage.uiMetrics.playerY,
      },
      peer: {
        x: burstPeerStage.uiMetrics.playerX,
        y: burstPeerStage.uiMetrics.playerY,
      },
    },
    burstSpawnCount: burstSpawns.length,
    burstAuthority: {
      inactivePresentation: inactiveBurstPresentation,
      activePresentation: activeBurstPresentation,
      serverDamage: burstDamageEvent,
      browserDamage: burstBrowserDamage,
    },
    burstStageSettlement,
    thorRepelStaging,
    burstLaneRestagings,
    lastBurstLaneRestaging,
    burstDoorWindow,
    rocketEvent,
    final: {
      actor: compactMetrics(final[0].uiMetrics),
      peer: compactMetrics(final[1].uiMetrics),
      observer: compactMetrics(final[2].uiMetrics),
      actorLocalGameplayState: final[0].uiMetrics.localGameplayState,
      peerLocalGameplayState: final[1].uiMetrics.localGameplayState,
    },
  };
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  const initialProvenance = collectArtifactProvenance({ runStartedAt, e2eScript, serverBinary, wasm: wasmPath, clientEntry });
  const serverPort = await freePort();
  const actorPort = await freePort();
  const peerPort = await freePort();
  const observerPort = await freePort();
  const staticServer = await startStaticServer();
  // The returning Burst uses a specific authored door and a truly stationary
  // peer. Declare that Chaos scenario without moving/respawning the witness
  // or changing the map, balance, weapon timers or three-role separation.
  // Extreme retains its original authored Spawn region and zero inventory.
  const spawnFixture = arena === "chaos-zone" ? createSpawnRegionFixture({
    zoneId: arena, spawnRegion: { x: 512, y: 512, radius: 0 },
  }) : null;
  const server = startRealServer(serverPort, { profile: serverProfile,
    ...(spawnFixture ? { zoneRoot: spawnFixture.root } : {}), env: {
    ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT: String(maxedSpawnLoadout),
    ...(spawnFixture ? { ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" } : {}),
  } });
  const peerChrome = startChrome(peerPort, "special-peer");
  const observerChrome = startChrome(observerPort, "special-observer");
  const actorChrome = firefoxActor ? null : startChrome(actorPort, "special-actor");
  const firefoxDriver = firefoxActor ? startGeckodriver(actorPort, "special-firefox") : null;
  let actor = null;
  let peer = null;
  let observer = null;
  let actorUrl = null;
  let peerUrl = null;
  let observerUrl = null;

  const extraLogs = () => ({
    "spawn-fixture": JSON.stringify(spawnFixture, null, 2),
    "peer-chrome": peerChrome.logs,
    "observer-chrome": observerChrome.logs,
    ...(actorChrome ? { "actor-chrome": actorChrome.logs } : {}),
    ...(firefoxDriver ? { geckodriver: firefoxDriver.logs } : {}),
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await Promise.all([
      waitForChrome(peerChrome),
      waitForChrome(observerChrome),
      actorChrome ? waitForChrome(actorChrome) : waitForGeckodriver(firefoxDriver),
    ]);
    if (firefoxDriver) await createSession(firefoxDriver);

    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1`;
    actorUrl = `${pageBase}&name=specialactor`;
    peerUrl = `${pageBase}&name=specialpeer`;
    observerUrl = `${pageBase}&name=specialobs`;
    // Bootstrap each active role through its lifecycle before adding the
    // passive observer. A third initial spectator can otherwise receive its
    // map before its self lifecycle baseline while two existing clients are
    // already publishing snapshots, leaving ship-selection edges correctly
    // closed by the gameplay authority gate.
    peer = await openChromeClient(peerChrome, peerUrl);
    await waitForZoneReady(peer, "special peer");
    await enterReadyShip(peer, "special peer");
    actor = actorChrome
      ? await openChromeClient(actorChrome, actorUrl)
      : await openFirefoxSpecialClient(firefoxDriver, actorUrl);
    await waitForZoneReady(actor, "special actor");
    await enterReadyShip(actor, "special actor");
    observer = await openChromeClient(observerChrome, observerUrl);
    await waitForZoneReady(observer, "special observer");
    await enterReadyShip(observer, "special observer");
    await Promise.all([
      waitForPlayersReady(actor, peer, server),
      waitForPlayersReady(actor, observer, server),
    ]);

    const [actorStart, peerStart, observerStart] = await Promise.all([
      actor.pageState(),
      peer.pageState(),
      observer.pageState(),
    ]);
    const actorId = summarizeClient(actorStart).selfId;
    const peerId = summarizeClient(peerStart).selfId;
    const observerId = summarizeClient(observerStart).selfId;
    if (
      actorId == null ||
      peerId == null ||
      observerId == null ||
      new Set([actorId, peerId, observerId]).size !== 3
    ) {
      throw new Error(`missing distinct player ids: ${JSON.stringify({
        actorId,
        peerId,
        observerId,
      })}`);
    }

    const statusResponse = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`, {
      signal: AbortSignal.timeout(10_000),
    });
    if (!statusResponse.ok) throw new Error(`special-weapon fixture status HTTP ${statusResponse.status}`);
    const status = await statusResponse.json();
    if (status.maxed_spawn_loadout !== maxedSpawnLoadout || status.zone !== arena) {
      throw new Error(`special-weapon fixture loadout/zone mismatch: ${JSON.stringify({
        arena, maxedSpawnLoadout, observedZone: status.zone,
        observedMaxedSpawnLoadout: status.maxed_spawn_loadout,
      })}`);
    }
    const fixture = { profile: maxedSpawnLoadout ? "maximum" : "reference",
      maxedSpawnLoadout, observedMaxedSpawnLoadout: status.maxed_spawn_loadout,
      arenaSettings: spawnFixture ? "only declared Spawn fields changed" : "unmodified",
      spawnFixture };
    const scenario = await runSpecialWeaponScenario({
      actor,
      peer,
      observer,
      server,
      actorId,
      peerId,
      observerId,
    });
    const securityAudit = assertSecurityRejectAudit(server);
    const dataChannelDeliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: Object.values(extraLogs()),
      // Every joined role must leave before the drain is asserted: the audit
      // waits for the server to close all three RTC sessions. The parenthesized
      // comma form is one expression, so both the Chrome DevTools evaluator
      // and the Firefox `return (...)` script wrapper accept it.
      disconnect: async () => {
        await Promise.all([actor, peer, observer].map((client) =>
          client.evaluate("(window.__zubspaceSendDisconnectForTest?.(), true)")));
      },
    });
    const finalProvenance = await collectArtifactProvenanceAsync({ runStartedAt, e2eScript, serverBinary, wasm: wasmPath, clientEntry });
    assertArtifactProvenanceMatches(initialProvenance, finalProvenance);
    const successArtifacts = successArtifactDir
      ? await saveBrowserE2eArtifacts({
          artifactDir: successArtifactDir,
          server,
          staticServer,
          urls: [actorUrl, peerUrl, observerUrl],
          clients: [
            { label: firefoxActor ? "actor-firefox" : "actor-chrome", client: actor },
            { label: "peer-chrome", client: peer },
            { label: "observer-chrome", client: observer },
          ],
          extraLogs: extraLogs(),
          provenance: finalProvenance,
        })
      : null;
    for (const client of [actor, peer, observer]) {
      assertRendererConsoleClean(browserConsoleEvidence(client));
    }
    const result = {
      ok: true,
      transport: "rtc",
      browserPair,
      actorBrowser: firefoxActor ? "firefox" : "chrome",
      arena,
      fixture,
      securityAudit,
      dataChannelDeliveryAudit,
      provenance: finalProvenance,
      scenario,
      successArtifacts,
    };
    if (successArtifactDir) {
      const resultPath = path.join(successArtifactDir, "result.json");
      fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
      validateArtifactPathReferences(result, resultPath);
    }
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifactDir = path.join("/tmp", `zubspace-special-weapons-e2e-${Date.now()}-${process.pid}`);
    const artifacts = await saveBrowserE2eArtifacts({
      artifactDir,
      provenance: await collectArtifactProvenanceAsync({ runStartedAt, e2eScript, serverBinary, wasm: wasmPath, clientEntry }),
      server,
      staticServer,
      urls: [actorUrl, peerUrl, observerUrl],
      clients: [
        { label: firefoxActor ? "actor-firefox" : "actor-chrome", client: actor },
        { label: "peer-chrome", client: peer },
        { label: "observer-chrome", client: observer },
      ],
      extraLogs: extraLogs(),
    });
    error.message = `${error.message}\n${browserE2eFailureFooter(artifacts)}\n\nzubspace_server logs:\n${server.logs.join("").trim()}`;
    throw error;
  } finally {
    await actor?.close().catch(() => {});
    await peer?.close().catch(() => {});
    await observer?.close().catch(() => {});
    await Promise.all([
      stopProcess(peerChrome.child),
      stopProcess(observerChrome.child),
      actorChrome ? stopProcess(actorChrome.child) : Promise.resolve(),
      firefoxDriver ? stopProcess(firefoxDriver.child) : Promise.resolve(),
      stopProcess(server.child),
    ]).catch(() => {});
    fs.rmSync(peerChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    fs.rmSync(observerChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    if (actorChrome) {
      fs.rmSync(actorChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    }
    staticServer.server.close();
    if (spawnFixture) fs.rmSync(spawnFixture.root, { recursive: true, force: true });
  }
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;

if (isMain) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exit(1);
  });
}
