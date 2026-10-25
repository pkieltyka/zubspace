#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

import {
  createSession,
  deleteSession,
  execute,
  firefoxKeyDown,
  firefoxKeyUp,
  firefoxArtifactClient,
  freePort,
  metricsState as firefoxMetricsState,
  openClient,
  releaseFirefoxKeys,
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
import {
  assertDataChannelDeliveryAudit,
  assertSecurityRejectAudit,
  driveClientToWorldTarget,
  compactMetrics,
  openChromeClient,
  orientClientForProtocolDirection,
  parseServerDamageEvents,
  startChrome,
  waitForChrome,
  waitUntil,
} from "./combat_replay.mjs";
import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import {
  artifactFileManifest,
  createAtomicArtifactDirectory,
  validateArtifactPathReferences,
} from "./artifact_paths.mjs";
import { assertCompleteQaTraceCaptures, captureBrowserArtifactState } from "./qa_trace_capture.mjs";
import { writeQaTraceArtifact, writeQaTraceFailureResult } from "./qa_trace_artifacts.mjs";
import { predictionTracksAuthoritativeSnapshot } from "./energy_recovery_evidence.mjs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";

const browser = process.env.ZUBSPACE_E2E_BROWSER || "chrome";
const arena = "chaos-zone";
process.env.ZUBSPACE_SERVER_SIM_OBSERVE ||= "1";
const runStartedAt = new Date().toISOString();
const e2eScript = "tests/e2e/energy_recovery.mjs";
const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const target = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
  path.join("/tmp", `zubspace-energy-recovery-${browser}-${Date.now()}-${process.pid}`);
const CHAOS_RECHARGE_PER_SECOND = 115;
// Chaos level-3 bullets deal 400 energy. Keep hostile damage in a deep-red,
// nonlethal band which one discrete authored hit can actually reach; the
// independent held-fire half above still requires the exact low-200s band.
const HOSTILE_DEEP_RED_ENERGY_MIN = 150;
const HOSTILE_DEEP_RED_ENERGY_MAX = 400;

if (!new Set(["chrome", "firefox"]).has(browser)) {
  throw new Error("ZUBSPACE_E2E_BROWSER must be chrome or firefox");
}

function parseTraceRecords(server, marker) {
  return server.logs.join("").split("\n")
    .filter((line) => line.includes(marker))
    .map((line) => {
      const record = {};
      for (const match of line.matchAll(/(\w+)=("[^"]*"|\S+)/g)) {
        const raw = match[2];
        if (raw === "true" || raw === "false") record[match[1]] = raw === "true";
        else if (raw !== "" && Number.isFinite(Number(raw))) record[match[1]] = Number(raw);
        else record[match[1]] = raw.replace(/^"|"$/g, "");
      }
      return record;
    });
}

function authorityReady(state) {
  const metrics = state?.uiMetrics || {};
  const local = metrics.localGameplayState || {};
  return metrics.playerAlive === true && metrics.serverPlayerAlive === true &&
    Number.isInteger(metrics.selfPlayerId) && Number.isInteger(metrics.playerLifeSeq) &&
    local.zoneFilesReady === true && local.zoneMapReady === true &&
    local.pendingShipResetPosition === false &&
    (local.lifecycleBaselineReceiptsMatched ?? 0) >= 1 &&
    (local.lifecycleBaselineReceiptsUnmatched ?? 0) === 0;
}

async function openActor(staticPort, serverPort, label, actorBrowser = browser) {
  const url = `http://127.0.0.1:${staticPort}/index.html?rtc=${encodeURIComponent(`http://127.0.0.1:${serverPort}/v1/rtc/offer`)}&arena=${arena}&packetlog=1&trace=1&name=${label}`;
  if (actorBrowser === "chrome") {
    const process = startChrome(await freePort(), label);
    await waitForChrome(process);
    const client = await openChromeClient(process, url);
    return {
      process,
      browserLogs: process.logs,
      metricsState: () => client.metricsState(),
      pageState: () => client.pageState(),
      evaluate: (expression) => client.evaluate(expression),
      freezeQaTrace: () => client.freezeQaTrace(),
      press: (key, holdMs = 80) => client.press(key, holdMs),
      keyDown: (key) => client.keyDown(key),
      keyUp: (key) => client.keyUp(key),
      releaseKeys: () => Promise.all([
        "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "w", "a", "s", "d",
        "Shift", "Control", "x",
      ].map((key) => client.keyUp(key).catch(() => {}))),
      screenshot: async () => Buffer.from(await client.screenshot(), "base64"),
      disconnect: () => client.evaluate("window.__zubspaceSendDisconnectForTest?.(); true"),
      close: () => client.close(),
    };
  }
  const process = startGeckodriver(await freePort(), `${label}-firefox`);
  await waitForGeckodriver(process);
  await createSession(process);
  await openClient(process, url);
  return {
    process,
    browserLogs: process.logs,
    // Feedback loops need only current authority/energy/movement state. The
    // full page snapshot serializes several bounded multi-megabyte histories
    // and can stall debug Firefox long enough to consume the physical thrust
    // deadline while the ship is visibly accelerating correctly.
    metricsState: () => firefoxMetricsState(process),
    ...firefoxArtifactClient(process),
    press: async (key, holdMs = 80) => {
      await firefoxKeyDown(process, key);
      try {
        await sleep(holdMs);
      } finally {
        await firefoxKeyUp(process, key).catch(() => {});
      }
    },
    keyDown: (key) => firefoxKeyDown(process, key),
    keyUp: (key) => firefoxKeyUp(process, key),
    releaseKeys: () => releaseFirefoxKeys(process),
    screenshot: async () => Buffer.from(await screenshot(process), "base64"),
    disconnect: () => execute(process, "window.__zubspaceSendDisconnectForTest?.(); return true;"),
    close: () => deleteSession(process),
  };
}

async function returnToSafety(actor) {
  const deadline = Date.now() + 12_000;
  while (Date.now() < deadline) {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    if (metrics.playerAlive !== true || metrics.serverPlayerAlive !== true) break;
    if (metrics.inSafeZone === true) return metrics;
    // This scenario never rotates from the canonical spawn heading (forward
    // is negative Y). Aim at a slow proportional return velocity instead of
    // thrusting solely from position: a max-speed ship can otherwise cross a
    // narrow green between two browser polls forever.
    const desiredVelocityY = Math.max(-80, Math.min(80, (8_200 - metrics.playerY) * 1.5));
    const key = desiredVelocityY - metrics.playerVelocityY < 0 ? "ArrowUp" : "ArrowDown";
    await actor.keyDown(key);
    await sleep(55);
    await actor.keyUp(key);
    await sleep(15);
  }
  throw new Error("timed out waiting for damaged pilot returns to authored safety");
}

async function sampleRecovery(actor, label, lowEnergyEvidence) {
  const samples = [];
  const first = (await actor.metricsState()).uiMetrics || {};
  const maxEnergy = first.playerMaxEnergy;
  const startedAt = Date.now();
  const firstLocal = first.localGameplayState || {};
  // Retain the exact low-energy observation that admitted this recovery.  A
  // fresh Firefox metrics poll can otherwise take close to a second during a
  // headed debug run, by which point a correct 115/s recharge has already
  // crossed the red-energy proof threshold.  Subsequent samples remain real
  // wall-clock observations; this simply gives their slope its true origin.
  samples.push({
    elapsedMs: 0,
    predicted: first.playerPredictedEnergy,
    server: first.serverPlayerEnergy,
    visible: first.visibleEnergyDigits,
    rechargePerSecond: firstLocal.rechargePerSecond,
    inputBits: firstLocal.inputBits,
    pendingWeapon: first.pendingWeapon,
    latestServerTick: first.latestServerTick,
    latestSnapshotServerTick: first.latestSnapshotServerTick,
    predictionTolerance: firstLocal.selfEnergyPredictionTolerance,
    velocityX: first.playerVelocityX,
    velocityY: first.playerVelocityY,
  });
  await waitUntil(`${label} reaches full energy at the authored Chaos rate`, async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    const local = metrics.localGameplayState || {};
    samples.push({
      elapsedMs: Date.now() - startedAt,
      predicted: metrics.playerPredictedEnergy,
      server: metrics.serverPlayerEnergy,
      visible: metrics.visibleEnergyDigits,
      rechargePerSecond: local.rechargePerSecond,
      inputBits: local.inputBits,
      pendingWeapon: metrics.pendingWeapon,
      latestServerTick: metrics.latestServerTick,
      latestSnapshotServerTick: metrics.latestSnapshotServerTick,
      predictionTolerance: local.selfEnergyPredictionTolerance,
      velocityX: metrics.playerVelocityX,
      velocityY: metrics.playerVelocityY,
    });
    return metrics.playerAlive === true && metrics.serverPlayerAlive === true &&
      metrics.serverPlayerEnergy >= maxEnergy - 1 &&
      metrics.visibleEnergyDigits >= maxEnergy - 1;
  }, 30_000);
  const usable = samples.filter((sample) => Number.isFinite(sample.server));
  // A Chaos bomb can legitimately bounce back into its owner well after the
  // fire key is released. That damage is not slow recharge: start the clean
  // timing interval at the final authoritative energy drop and require the
  // remainder of the curve to be monotonic. The retained prefix makes any
  // delayed damage visible in the artifact instead of hiding it.
  let cleanStartIndex = 0;
  for (let index = 1; index < usable.length; index += 1) {
    if (usable[index].server + 2 < usable[index - 1].server) cleanStartIndex = index;
  }
  const clean = usable.slice(cleanStartIndex);
  const start = clean[0];
  const end = clean.at(-1);
  const expectedMs = (maxEnergy - start.server) / CHAOS_RECHARGE_PER_SECOND * 1_000;
  const elapsedMs = end.elapsedMs - start.elapsedMs;
  const fastCoast = clean.filter((sample) =>
    Math.hypot(sample.velocityX ?? 0, sample.velocityY ?? 0) > 215
  );
  const fastCoastAdvance = fastCoast.length >= 2
    ? fastCoast.at(-1).server - fastCoast[0].server
    : 0;
  const fastCoastElapsedMs = fastCoast.length >= 2
    ? fastCoast.at(-1).elapsedMs - fastCoast[0].elapsedMs
    : 0;
  const fastCoastMaximumSampleGapMs = fastCoast.slice(1).reduce(
    (maximum, sample, index) => Math.max(
      maximum,
      sample.elapsedMs - fastCoast[index].elapsedMs,
    ),
    0,
  );
  // The wall-clock interval ends when a browser poll first observes full
  // energy, after the authoritative transition itself. Discount at most one
  // measured polling interval, then require the complete authored slope over
  // the remaining fast-coast window. A fixed tolerance here was both less
  // precise and smaller than an ordinary ~100 ms metrics poll.
  const fastCoastMinimumExpectedAdvance =
    CHAOS_RECHARGE_PER_SECOND *
      Math.max(0, fastCoastElapsedMs - fastCoastMaximumSampleGapMs) / 1_000 -
    2;
  if (
    clean.length < 4 ||
    // Prove that the replay genuinely entered red energy.  If a Chaos bomb
    // later bounces into its owner, the damage-free slope begins at the first
    // browser poll after that final drop and may already be above 425; the
    // retained prefix still contains the original exact low-energy state.
    (lowEnergyEvidence > 425 && usable.every((sample) => sample.server > 425)) ||
    clean.some((sample) => sample.rechargePerSecond !== CHAOS_RECHARGE_PER_SECOND) ||
    elapsedMs > expectedMs + 1_750 ||
    clean.some((sample, index) => index > 0 && sample.server + 2 < clean[index - 1].server) ||
    clean.some((sample) =>
      sample.visible > sample.server + 1 ||
      !predictionTracksAuthoritativeSnapshot(sample, CHAOS_RECHARGE_PER_SECOND)
    ) ||
    (label === "weapon-spend" && (
      fastCoast.length < 4 ||
      fastCoastElapsedMs < 500 ||
      fastCoastAdvance < fastCoastMinimumExpectedAdvance
    ))
  ) {
    throw new Error(`${label} recovery diverged from authored 115 energy/sec: ${JSON.stringify({ lowEnergyEvidence, expectedMs, elapsedMs, cleanStartIndex, fastCoastElapsedMs, fastCoastMaximumSampleGapMs, fastCoastMinimumExpectedAdvance, fastCoastAdvance, samples: usable })}`);
  }
  return {
    label,
    maxEnergy,
    lowEnergyEvidence,
    expectedMs,
    elapsedMs,
    delayedDamageSamples: usable.slice(0, cleanStartIndex),
    fastCoast: {
      elapsedMs: fastCoastElapsedMs,
      maximumSampleGapMs: fastCoastMaximumSampleGapMs,
      minimumExpectedAdvance: fastCoastMinimumExpectedAdvance,
      energyAdvance: fastCoastAdvance,
      samples: fastCoast,
    },
    samples: clean,
  };
}

async function establishFastCoast(actor, server, playerId, lifeSeq, {
  energyCeiling = null,
  label = "ordinary no-Shift thrust reaches upgraded coasting speed",
  preferredThrustKey = null,
} = {}) {
  const offset = parseTraceRecords(server, "human authority input trace").length;
  const initial = (await actor.metricsState()).uiMetrics || {};
  // Prior movement or an authoritative collision can leave the ship coasting
  // in either authored thrust axis.
  // Reinforce the existing direction so this speed-based hidden-drain proof
  // does not spend the short red-energy window braking through zero first.
  // Neither direction includes Shift, which is the authority distinction the
  // former velocity-inferred Afterburner bug violated.
  const velocityY = initial.playerVelocityY ?? 0;
  const thrustKey = preferredThrustKey ?? (
    Math.abs(velocityY) > 215
      ? (velocityY > 0 ? "ArrowDown" : "ArrowUp")
      // At sub-coasting speed, aim back into the center of this authored
      // vertical Chaos corridor instead of reinforcing a just-completed wall
      // collision. The y=8,600 split is fixture geometry, not hit tolerance.
      : ((initial.playerY ?? 8_200) < 8_600 ? "ArrowDown" : "ArrowUp")
  );
  await actor.keyDown(thrustKey);
  let accelerated = null;
  try {
    await waitUntil(label, async () => {
      accelerated = (await actor.metricsState()).uiMetrics || {};
      return accelerated.playerAlive === true && accelerated.serverPlayerAlive === true &&
        accelerated.playerLifeSeq === lifeSeq &&
        (energyCeiling === null || accelerated.playerEnergy <= energyCeiling) &&
        Math.hypot(accelerated.playerVelocityX ?? 0, accelerated.playerVelocityY ?? 0) > 240;
    }, 4_000);
  } finally {
    await actor.keyUp(thrustKey).catch(() => {});
  }
  await waitUntil("ordinary Forward release owns a fast neutral server witness", async () => {
    return parseTraceRecords(server, "human authority input trace").slice(offset)
      .some((record) =>
        record.player_id === playerId && record.life_seq === lifeSeq &&
        (record.input_bits & 0x13) === 0 &&
        Math.hypot((record.velocity_x ?? 0) / 10, (record.velocity_y ?? 0) / 10) > 215
      );
  }, 3_000);
  return {
    thrustKey,
    initialEnergy: initial.playerEnergy,
    initialServerEnergy: initial.serverPlayerEnergy,
    initialVelocityX: initial.playerVelocityX,
    initialVelocityY: initial.playerVelocityY,
    initialX: initial.playerX,
    initialY: initial.playerY,
    energy: accelerated.playerEnergy,
    serverEnergy: accelerated.serverPlayerEnergy,
    velocityX: accelerated.playerVelocityX,
    velocityY: accelerated.playerVelocityY,
    speed: Math.hypot(accelerated.playerVelocityX ?? 0, accelerated.playerVelocityY ?? 0),
  };
}

async function runWeaponSpend(staticPort) {
  const serverPort = await freePort();
  const server = startRealServer(serverPort, { zone: arena, botCount: 0 });
  let actor = null;
  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    actor = await openActor(staticPort, serverPort, `gun${browser}`);
    let state = null;
    await waitUntil(`${browser} weapon-spend authority`, async () => {
      state = await actor.metricsState();
      return authorityReady(state);
    }, 60_000);
    const playerId = state.uiMetrics.selfPlayerId;
    const lifeSeq = state.uiMetrics.playerLifeSeq;
    // The fast-coast assertion below requires a real clear corridor. Random
    // entry elsewhere can reach speed240 and immediately strike terrain,
    // leaving a correct recharge curve without its required500ms fast window.
    // Establish the original vertical fixture before spending energy, using
    // genuine routed flight and a north-facing stopped pose. No measured
    // energy, acceleration, speed or coasting deadline is extended.
    const approach = await navigateAuthoredTarget(actor, { x: 8200, y: 8300 }, server, {
      timeoutMs: 90_000, arrivalRadiusPx: 12,
      maximumDesiredSpeed: 100, desiredSpeedScale: 0.5,
    });
    const fineStage = await driveClientToWorldTarget(actor, { x: 8200, y: 8300 },
      15_000, true, 4);
    const facing = await orientClientForProtocolDirection(actor, 0, -1, 5_000);
    const setup = { approach, fineStage, facing };
    if (!approach.reached || !fineStage.reached || !facing.oriented) {
      throw new Error(`weapon-spend pilot could not enter its authored corridor: ${JSON.stringify(setup)}`);
    }
    // Keep the ship stationary during the held-fire spend. Letting it coast
    // through the whole 11-second firing window carries it across the narrow
    // authored spawn corridor, so the post-spend acceleration can spend its
    // short red-energy window recovering from a wall bounce. The behavior this
    // case owns is still proved below with real no-Shift thrust, a neutral
    // server input witness, and the unchanged strict red-energy ceiling.
    const preSpendState = (await actor.metricsState()).uiMetrics || {};
    const traceOffset = parseTraceRecords(server, "human authority input trace").length;
    let spend = null;
    // Spend with ordinary bullets, matching the reported gameplay regression
    // and keeping movement independent of bomb recoil, delayed self-splash,
    // and nearby wall bounces. Those are covered by the dedicated bomb E2E;
    // this scenario owns the recharge and no-Shift coasting invariant.
    await actor.keyDown("Control");
    try {
      await waitUntil("held real Ctrl spends Chaos energy into the low 200s", async () => {
        const records = parseTraceRecords(server, "human authority input trace").slice(traceOffset)
          .filter((record) => record.player_id === playerId && record.life_seq === lifeSeq);
        const bullets = records.filter((record) => [1, 2].includes(record.accepted_weapon & 0x1f));
        const latest = records.at(-1);
        const reached = bullets.length >= 8 && latest?.energy >= 150 && latest.energy <= 225;
        if (reached) {
          spend = {
            acceptedBullets: bullets.length,
            terminalServerTick: latest.server_tick,
            terminalInputSequence: latest.input_sequence,
            terminalEnergy: latest.energy,
            terminalVelocityX: (latest.velocity_x ?? 0) / 10,
            terminalVelocityY: (latest.velocity_y ?? 0) / 10,
          };
        }
        return reached;
      // Each shot remains application-receipt gated, so a debug browser can
      // legitimately skip a cooldown-ready frame while the preceding offer
      // is still in flight. This is a fail-only ceiling; the retained trace
      // still proves the exact accepted shots and low-energy terminal state.
      }, 20_000);
    } finally {
      await actor.keyUp("Control").catch(() => {});
    }
    const releaseOffset = parseTraceRecords(server, "human authority input trace").length;
    await waitUntil("released Ctrl reaches an authoritative neutral weapon record", async () => {
      return parseTraceRecords(server, "human authority input trace").slice(releaseOffset)
        .some((record) => record.player_id === playerId && record.life_seq === lifeSeq && record.reported_weapon === 0);
    }, 3_000);
    const fastCoast = await establishFastCoast(actor, server, playerId, lifeSeq, {
      energyCeiling: 425,
      label: "post-spend coasting remains fast while energy stays red",
    });
    const recovery = await sampleRecovery(actor, "weapon-spend", spend.terminalEnergy);
    const postRelease = parseTraceRecords(server, "human authority input trace").slice(releaseOffset)
      .filter((record) => record.player_id === playerId && record.life_seq === lifeSeq);
    const neutralIndex = postRelease.findIndex((record) => record.reported_weapon === 0);
    if (postRelease.slice(neutralIndex).some((record) => record.accepted_weapon !== 0)) {
      throw new Error("weapon input remained active after the genuine Ctrl key release");
    }
    const finalState = await captureBrowserArtifactState(actor);
    assertCompleteQaTraceCaptures([finalState.qaTraceCapture]);
    const selfDamage = finalState.damageTrace.filter((entry) =>
      entry.forSelf === true && entry.attackerId === playerId && entry.victimId === playerId
    );
    if (selfDamage.length !== 0) {
      throw new Error(`ordinary bullet energy spend unexpectedly damaged its owner: ${JSON.stringify(selfDamage)}`);
    }
    const screenshotBytes = await actor.screenshot();
    const securityAudit = assertSecurityRejectAudit(server);
    const deliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: [actor.browserLogs],
      disconnect: actor.disconnect,
    });
    await sleep(500);
    return { server, actor, serverPort, playerId, lifeSeq, setup, preSpendState, spend, fastCoast, recovery, selfDamage, finalState, screenshotBytes, securityAudit, deliveryAudit,
      qaCaptures: [{ label: "weapon-browser", state: finalState, serverLogs: server.logs }] };
  } catch (error) {
    error.serverLog = server.logs.join("");
    error.qaCaptures = [{ label: "weapon-browser", state: await captureBrowserArtifactState(actor), serverLogs: server.logs }];
    throw error;
  } finally {
    await actor?.releaseKeys().catch(() => {});
    await actor?.close().catch(() => {});
    await stopProcess(actor?.process?.child).catch(() => {});
    await stopProcess(server.child).catch(() => {});
  }
}

async function runHostileDamage(staticPort) {
  const serverPort = await freePort();
  const server = startRealServer(serverPort, { zone: arena, botCount: 0 });
  let actor = null;
  let attacker = null;
  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    [actor, attacker] = await Promise.all([
      openActor(staticPort, serverPort, `hurt${browser}`),
      // Keep the measured browser under test as Chrome/Firefox, but use CDP
      // for the hostile peer so five real key taps arrive faster than the
      // authored recharge. Firefox WebDriver command latency alone is long
      // enough to refill the victim between taps and cannot exercise low
      // energy without inventing raw packet input.
      openActor(staticPort, serverPort, `rechargeAttk`, "chrome"),
    ]);
    let victimState = null;
    let attackerState = null;
    await waitUntil(`${browser} two-player hostile-damage authority`, async () => {
      [victimState, attackerState] = await Promise.all([actor.metricsState(), attacker.metricsState()]);
      return authorityReady(victimState) && authorityReady(attackerState);
    }, 60_000);
    const playerId = victimState.uiMetrics.selfPlayerId;
    const attackerId = attackerState.uiMetrics.selfPlayerId;
    const lifeSeq = victimState.uiMetrics.playerLifeSeq;
    if (playerId === attackerId) throw new Error("hostile recharge peers reused one player id");
    // Stock Chaos spawns are random. Travel to the fixed combat neighborhood
    // through full-hull-clear, door/wormhole-aware routes before the original
    // short precision stage. This setup flight is outside energy measurement;
    // the 15s fine approach, ordinary-gun spacing and recharge assertions stay
    // unchanged. Retain both routes with the measured fixture.
    const approaches = await Promise.all([
      navigateAuthoredTarget(actor, { x: 8064, y: 8225 }, server, {
        timeoutMs: 90_000, arrivalRadiusPx: 12,
        maximumDesiredSpeed: 100, desiredSpeedScale: 0.5,
      }),
      navigateAuthoredTarget(attacker, { x: 8138, y: 8190 }, server, {
        timeoutMs: 90_000, arrivalRadiusPx: 12,
        maximumDesiredSpeed: 100, desiredSpeedScale: 0.5,
      }),
    ]);
    if (!approaches.every(approach => approach.reached)) {
      throw new Error(`hostile recharge peers missed their authored approach: ${JSON.stringify(approaches)}`);
    }
    const [victimRoute, attackerRoute] = await Promise.all([
      // These are staging neighborhoods, not point-collision assertions. A
      // 12 px stopped arrival is still inside one ship hull; the measured
      // inter-ship lane below remains the authoritative fixture boundary.
      driveClientToWorldTarget(actor, { x: 8064, y: 8225 }, 15_000, true, 12),
      driveClientToWorldTarget(attacker, { x: 8138, y: 8190 }, 15_000, true, 12),
    ]);
    if (victimRoute.reached !== true || attackerRoute.reached !== true) {
      throw new Error(`hostile recharge peers could not enter the authored combat fixture: ${JSON.stringify({ victimRoute, attackerRoute })}`);
    }
    [victimState, attackerState] = await Promise.all([actor.metricsState(), attacker.metricsState()]);
    const victimMetrics = victimState.uiMetrics || {};
    const attackerMetrics = attackerState.uiMetrics || {};
    const fixtureDx = victimMetrics.playerX - attackerMetrics.playerX;
    const fixtureDy = victimMetrics.playerY - attackerMetrics.playerY;
    const fixtureDistance = Math.hypot(fixtureDx, fixtureDy);
    if (
      victimMetrics.inSafeZone !== false || attackerMetrics.inSafeZone !== false ||
      fixtureDistance < 40 || fixtureDistance > 140
    ) {
      throw new Error(`hostile recharge fixture is not a valid ordinary-gun lane: ${JSON.stringify({
        fixtureDistance,
        victim: compactMetrics(victimMetrics),
        attacker: compactMetrics(attackerMetrics),
      })}`);
    }
    const aim = await orientClientForProtocolDirection(
      attacker,
      fixtureDx,
      fixtureDy,
      5_000,
    );
    if (aim.oriented !== true) {
      throw new Error(`hostile recharge attacker could not aim through the measured victim: ${JSON.stringify({ aim, fixtureDistance })}`);
    }
    const fixture = {
      approaches,
      victimRoute,
      attackerRoute,
      distance: fixtureDistance,
      desiredDirection: aim.desiredDirection,
      direction: aim.direction,
    };
    let hit = null;
    for (let shot = 0; shot < 5; shot += 1) {
      const beforeCount = parseServerDamageEvents(server.logs).length;
      const beforeInput = parseTraceRecords(server, "human authority input trace").length;
      await attacker.keyDown("Control");
      let accepted = null;
      const acceptedDeadline = Date.now() + 2_000;
      try {
        while (Date.now() < acceptedDeadline && accepted == null) {
          accepted = parseTraceRecords(server, "human authority input trace").slice(beforeInput)
            .find((record) => record.player_id === attackerId &&
              record.life_seq === attackerState.uiMetrics.playerLifeSeq &&
              [1, 2].includes(record.accepted_weapon & 0x1f)) || null;
          if (accepted == null) await sleep(10);
        }
      } finally {
        await attacker.keyUp("Control").catch(() => {});
      }
      if (accepted == null) {
        throw new Error(`hostile genuine Control key did not produce an accepted gun record: ${JSON.stringify({
          shot,
          fixture,
          input: parseTraceRecords(server, "human authority input trace").slice(beforeInput),
        })}`);
      }
      await waitUntil(`hostile keyboard bullet ${shot + 1} reaches victim`, async () => {
        return parseServerDamageEvents(server.logs).slice(beforeCount).some((entry) =>
          entry.shooterId === attackerId && entry.victimId === playerId &&
          entry.victimLifeSeq === lifeSeq
        );
      }, 8_000);
      hit = parseServerDamageEvents(server.logs).slice(beforeCount).findLast((entry) =>
        entry.shooterId === attackerId && entry.victimId === playerId &&
        entry.victimLifeSeq === lifeSeq
      ) || hit;
      if (hit.explicitLethal === true || hit.energyAfter <= 0) {
        throw new Error(`hostile keyboard damage killed the recharge pilot: ${JSON.stringify({ shot, hit })}`);
      }
      if (
        hit.energyAfter >= HOSTILE_DEEP_RED_ENERGY_MIN &&
        hit.energyAfter <= HOSTILE_DEEP_RED_ENERGY_MAX
      ) break;
      // Waiting for this bullet's authoritative damage already spans several
      // neutral input records after the genuine key-up. An additional fixed
      // pause here only recharges the victim between otherwise exact hits and
      // makes the low-energy fixture skip from the 300s to nearly zero.
    }
    const low = (await actor.metricsState()).uiMetrics || {};
    if (
      hit?.energyAfter < HOSTILE_DEEP_RED_ENERGY_MIN ||
      hit?.energyAfter > HOSTILE_DEEP_RED_ENERGY_MAX
    ) {
      throw new Error(`hostile real-key damage did not reach deep-red energy: ${JSON.stringify({ hit, metrics: compactMetrics(low) })}`);
    }
    await attacker.releaseKeys().catch(() => {});
    const recovery = await sampleRecovery(actor, "hostile-damage", hit.energyAfter);
    const finalState = await captureBrowserArtifactState(actor);
    const attackerArtifactState = await captureBrowserArtifactState(attacker);
    assertCompleteQaTraceCaptures([finalState.qaTraceCapture, attackerArtifactState.qaTraceCapture]);
    const securityAudit = assertSecurityRejectAudit(server);
    const deliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: [actor.browserLogs, attacker.browserLogs],
      disconnect: () => Promise.all([actor.disconnect(), attacker.disconnect()]),
    });
    await sleep(500);
    return { server, actor, serverPort, playerId, lifeSeq, fixture, hit, recovery, finalState, securityAudit, deliveryAudit,
      qaCaptures: [
        { label: "damage-browser", state: finalState, serverLogs: server.logs },
        { label: "damage-attacker", state: attackerArtifactState, serverLogs: server.logs },
      ] };
  } catch (error) {
    error.serverLog = server.logs.join("");
    error.qaCaptures = [
      { label: "damage-browser", state: await captureBrowserArtifactState(actor), serverLogs: server.logs },
      { label: "damage-attacker", state: await captureBrowserArtifactState(attacker), serverLogs: server.logs },
    ];
    throw error;
  } finally {
    await actor?.releaseKeys().catch(() => {});
    await attacker?.releaseKeys().catch(() => {});
    await actor?.close().catch(() => {});
    await attacker?.close().catch(() => {});
    await stopProcess(actor?.process?.child).catch(() => {});
    await stopProcess(attacker?.process?.child).catch(() => {});
    await stopProcess(server.child).catch(() => {});
  }
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  const initialProvenance = collectArtifactProvenance({ runStartedAt, e2eScript });
  const artifactDirectory = createAtomicArtifactDirectory(target);
  const staticServer = await startStaticServer();
  let activeError = null;
  const completedQaCaptures = [];
  try {
    const weapon = await runWeaponSpend(staticServer.port);
    completedQaCaptures.push(...weapon.qaCaptures);
    const damage = await runHostileDamage(staticServer.port);
    completedQaCaptures.push(...damage.qaCaptures);
    const provenance = collectArtifactProvenance({ runStartedAt, e2eScript });
    assertArtifactProvenanceMatches(initialProvenance, provenance);
    fs.writeFileSync(path.join(artifactDirectory.temporary, "weapon-server.log"), weapon.server.logs.join(""));
    fs.writeFileSync(path.join(artifactDirectory.temporary, "damage-server.log"), damage.server.logs.join(""));
    fs.writeFileSync(path.join(artifactDirectory.temporary, "browser.png"), weapon.screenshotBytes);
    fs.writeFileSync(path.join(artifactDirectory.temporary, "weapon-browser-state.json"), `${JSON.stringify(weapon.finalState, null, 2)}\n`);
    fs.writeFileSync(path.join(artifactDirectory.temporary, "damage-browser-state.json"), `${JSON.stringify(damage.finalState, null, 2)}\n`);
    const qaTraceCaptureAudit = completedQaCaptures.map((capture) => writeQaTraceArtifact({
      directory: artifactDirectory.temporary, provenance, ...capture }));
    const result = {
      ok: true,
      browser,
      arena,
      authoredRechargePerSecond: CHAOS_RECHARGE_PER_SECOND,
      weapon: { playerId: weapon.playerId, lifeSeq: weapon.lifeSeq, setup: weapon.setup, preSpendState: compactMetrics(weapon.preSpendState), spend: weapon.spend, fastCoast: weapon.fastCoast, recovery: weapon.recovery, selfDamage: weapon.selfDamage, securityAudit: weapon.securityAudit, deliveryAudit: weapon.deliveryAudit },
      hostileDamage: { playerId: damage.playerId, lifeSeq: damage.lifeSeq, fixture: damage.fixture, hit: damage.hit, recovery: damage.recovery, securityAudit: damage.securityAudit, deliveryAudit: damage.deliveryAudit },
      final: compactMetrics(damage.finalState.uiMetrics),
      provenance,
      qaTraceCaptureAudit,
      artifactFiles: [],
    };
    const resultPath = path.join(artifactDirectory.temporary, "result.json");
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
    result.artifactFiles = artifactFileManifest(artifactDirectory.temporary, { exclude: ["result.json"] });
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
    validateArtifactPathReferences(result, resultPath);
    artifactDirectory.publish();
    console.log(JSON.stringify({ ok: true, browser, weapon: weapon.recovery, hostileDamage: damage.recovery }, null, 2));
  } catch (error) {
    activeError = error;
    artifactDirectory.cleanup();
    const failure = `${target}-failure`;
    fs.mkdirSync(failure, { recursive: false });
    fs.writeFileSync(path.join(failure, "error.txt"), `${error.stack || error.message}\n`);
    if (error.serverLog) fs.writeFileSync(path.join(failure, "server.log"), error.serverLog);
    writeQaTraceFailureResult({ directory: failure, provenance: initialProvenance,
      captures: [...completedQaCaptures, ...(error.qaCaptures || [])] });
    error.message = `${error.message}\nretained energy failure: ${failure}`;
    throw error;
  } finally {
    staticServer.server.close();
    if (activeError) await sleep(50);
  }
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
