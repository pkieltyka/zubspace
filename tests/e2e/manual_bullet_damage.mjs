#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  freePort,
  repoRoot,
  sleep,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";

import {
  assertServerDamageAccounting,
  browserE2eFailureFooter,
  compactMetrics,
  openChromeClient,
  parseServerDamageEvents,
  startChrome,
  waitForChrome,
  waitUntil,
  waitForCombatViewWeaponReady,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";

const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const bulletPressMs = Number(process.env.ZUBSPACE_E2E_MANUAL_BULLET_PRESS_MS || "30");
const damageSampleWindowMs = Number(
  process.env.ZUBSPACE_E2E_MANUAL_DAMAGE_WINDOW_MS || "3000",
);
const fullRegenRepeats = Number(process.env.ZUBSPACE_E2E_MANUAL_FULL_REGEN_REPEATS || "2");
const playerName = process.env.ZUBSPACE_E2E_MANUAL_PLAYER_NAME || "manualpilot";
// The server deliberately rejects duplicate handles. Keep both real clients
// distinct within the14-character limit, including a caller-provided prefix.
const playerNames = { alpha: `${playerName.slice(0, 13)}1`, beta: `${playerName.slice(0, 13)}2` };

function matchingEvents(server, sinceCount, shooterId, victimId) {
  return parseServerDamageEvents(server.logs)
    .slice(sinceCount)
    .filter((event) => event.shooterId === shooterId && event.victimId === victimId);
}

function bulletSelfDamageEvents(server, sinceCount) {
  return parseServerDamageEvents(server.logs)
    .slice(sinceCount)
    .filter((event) =>
      event.shooterId === event.victimId &&
      [1, 2, 15].includes(event.weapon & 0x001f)
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

function remotePlayerMetric(metrics, playerId) {
  return (metrics?.remotePlayers || []).find((player) => player.id === playerId) || null;
}

function sampledRemotePlayer(entry, playerId) {
  return (entry?.remotePlayers || []).find((player) => player.id === playerId) || null;
}

function sampledEnergy(entry, energyField, remotePlayerId = null) {
  if (remotePlayerId != null) {
    return sampledRemotePlayer(entry, remotePlayerId)?.energy;
  }
  return entry?.[energyField];
}

async function installManualMetricsSampler(client) {
  await client.evaluate(`(() => {
    window.__zubspaceManualE2eMetricsTrace = [];
    if (window.__zubspaceManualE2eSampler) {
      clearInterval(window.__zubspaceManualE2eSampler);
    }
    window.__zubspaceManualE2eSampler = setInterval(() => {
      const metrics = window.__zubspaceUiMetrics;
      if (!metrics) return;
      window.__zubspaceManualE2eMetricsTrace.push({
        at: Date.now(),
        selfPlayerId: metrics.selfPlayerId,
        playerEnergy: metrics.playerEnergy,
        playerPredictedEnergy: metrics.playerPredictedEnergy,
        localShipLabelEnergy: metrics.localShipLabelEnergy,
        visibleEnergyDigits: metrics.visibleEnergyDigits,
        playerMaxEnergy: metrics.playerMaxEnergy,
        serverPlayerEnergy: metrics.serverPlayerEnergy,
        playerAlive: metrics.playerAlive,
        serverPlayerAlive: metrics.serverPlayerAlive,
        playerRespawnTimer: metrics.playerRespawnTimer,
        lastNetworkDamageServerSeq: metrics.lastNetworkDamageServerSeq,
        lastNetworkDamageServerTick: metrics.lastNetworkDamageServerTick,
        lastNetworkDamagePlayerId: metrics.lastNetworkDamagePlayerId,
        lastNetworkDamageAttackerId: metrics.lastNetworkDamageAttackerId,
        lastNetworkDamageEnergyBefore: metrics.lastNetworkDamageEnergyBefore,
        lastNetworkDamageEnergy: metrics.lastNetworkDamageEnergy,
        lastNetworkDamageAmount: metrics.lastNetworkDamageAmount,
        lastNetworkDamageWeapon: metrics.lastNetworkDamageWeapon,
        lastNetworkDamageForSelf: metrics.lastNetworkDamageForSelf,
        lastSelfDamageServerSeq: metrics.lastSelfDamageServerSeq,
        lastSelfDamageServerTick: metrics.lastSelfDamageServerTick,
        lastSelfDamageAttackerId: metrics.lastSelfDamageAttackerId,
        lastSelfDamageEnergyBefore: metrics.lastSelfDamageEnergyBefore,
        lastSelfDamageEnergy: metrics.lastSelfDamageEnergy,
        lastSelfDamageAmount: metrics.lastSelfDamageAmount,
        lastSelfDamageWeapon: metrics.lastSelfDamageWeapon,
        remotePlayers: metrics.remotePlayers || [],
      });
      if (window.__zubspaceManualE2eMetricsTrace.length > 2400) {
        window.__zubspaceManualE2eMetricsTrace.splice(
          0,
          window.__zubspaceManualE2eMetricsTrace.length - 2400,
        );
      }
    }, 16);
    return true;
  })()`);
}

async function manualMetricsTrace(client) {
  return client.evaluate(`(() => window.__zubspaceManualE2eMetricsTrace || [])()`);
}

async function manualMetricsTraceLength(client) {
  return client.evaluate(`(() => (window.__zubspaceManualE2eMetricsTrace || []).length)()`);
}

function visibleSelfEnergyFull(metrics) {
  const max = metrics?.playerMaxEnergy ?? 1700;
  return (
    (metrics?.playerAlive === true) &&
    (metrics?.serverPlayerAlive === true) &&
    (metrics?.playerRespawnTimer ?? 0) <= 0 &&
    (metrics?.playerEnergy ?? 0) >= max - 1 &&
    (metrics?.visibleEnergyDigits ?? 0) >= max - 1 &&
    (metrics?.localShipLabelEnergy ?? 0) >= max - 1
  );
}

async function waitForManualVisibleFull(victim, label) {
  let full = null;
  await waitUntil(label, async () => {
    const state = await victim.metricsState();
    const metrics = state.uiMetrics || {};
    if (!visibleSelfEnergyFull(metrics)) {
      return false;
    }
    full = compactMetrics(metrics);
    return true;
  }, 15_000);
  return full;
}

function assertServerFullWhenHudLooksFull(metrics, label) {
  const max = metrics?.playerMaxEnergy ?? 1700;
  const problems = [];
  if (!visibleSelfEnergyFull(metrics)) {
    problems.push("visible HUD did not look full");
  }
  if ((metrics?.serverPlayerEnergy ?? 0) < max - 1) {
    problems.push("server-fed self energy was not full");
  }
  if (problems.length > 0) {
    throw new Error(`${label} manual HUD/full-energy mismatch: ${JSON.stringify({
      problems,
      metrics: compactMetrics(metrics),
    })}`);
  }
}

async function waitForManualPlayersReady(alpha, beta) {
  await waitUntil("both normal-url players assigned ids and visible", async () => {
    const [alphaState, betaState] = await Promise.all([
      alpha.metricsState(),
      beta.metricsState(),
    ]);
    const alphaMetrics = alphaState.uiMetrics || {};
    const betaMetrics = betaState.uiMetrics || {};
    return (
      alphaMetrics.selfPlayerId != null &&
      betaMetrics.selfPlayerId != null &&
      alphaMetrics.selfPlayerId !== betaMetrics.selfPlayerId &&
      (alphaMetrics.remoteVisiblePlayers ?? 0) >= 1 &&
      (betaMetrics.remoteVisiblePlayers ?? 0) >= 1 &&
      (alphaMetrics.playerRespawnTimer ?? 0) <= 0 &&
      (betaMetrics.playerRespawnTimer ?? 0) <= 0
    );
  }, 30_000);
}

async function waitForManualDamageHud({ victim, shooter, event, label }) {
  // Energy display is now fully live: after a hit the bar shows the post-hit
  // value for a single instant and then recharges immediately. We therefore
  // wait for the damage to be *registered* on both clients (server-owned
  // damage identity/result) rather than for the live bar to currently sit at
  // `energyAfter`; live energy may already have recharged by the poll.
  let sample = null;
  await waitUntil(`${label} manual HUD registered damage`, async () => {
    const [victimState, shooterState] = await Promise.all([
      victim.metricsState(),
      shooter.metricsState(),
    ]);
    const victimMetrics = victimState.uiMetrics || {};
    const shooterMetrics = shooterState.uiMetrics || {};
    const remote = remotePlayerMetric(shooterMetrics, event.victimId);
    const victimMatches =
      victimMetrics.lastSelfDamageServerSeq === event.damageSeq &&
      victimMetrics.lastSelfDamageServerTick === event.serverTick &&
      // `lastSelfDamageEnergy` is the server-owned hit result (always
      // `energyAfter`), a retained damage-identity field that does not
      // recharge -- unlike the live `visibleEnergyDigits` / ship label.
      Math.abs((victimMetrics.lastSelfDamageEnergy ?? -1) - event.energyAfter) <= 1 &&
      (victimMetrics.serverPlayerEnergy ?? Number.NaN) >= event.energyAfter - 2 &&
      (victimMetrics.playerEnergy ?? Number.NaN) >= event.energyAfter - 2;
    const shooterMatches =
      shooterMetrics.lastNetworkDamageServerSeq === event.damageSeq &&
      shooterMetrics.lastNetworkDamageServerTick === event.serverTick &&
      Math.abs((shooterMetrics.lastNetworkDamageEnergy ?? -1) - event.energyAfter) <= 1 &&
      remote &&
      (remote.serverEnergy ?? Number.NaN) >= event.energyAfter - 2 &&
      (remote.energy ?? Number.NaN) >= event.energyAfter - 2;
    if (victimMatches && shooterMatches) {
      sample = {
        victim: compactMetrics(victimMetrics),
        shooter: compactMetrics(shooterMetrics),
        shooterRemote: remote,
      };
      return true;
    }
    return false;
  }, 4_000);
  await sleep(damageSampleWindowMs);
  return sample;
}

async function assertManualDamageWindow({
  client,
  event,
  label,
  traceStart,
  energyField,
  remotePlayerId = null,
}) {
  const trace = (await manualMetricsTrace(client)).slice(traceStart);
  const matching = trace.filter((entry) => {
    if (remotePlayerId != null) {
      return (
        entry.lastNetworkDamageServerSeq === event.damageSeq &&
        entry.lastNetworkDamageServerTick === event.serverTick &&
        entry.lastNetworkDamagePlayerId === event.victimId &&
        entry.lastNetworkDamageAttackerId === event.shooterId
      );
    }
    return (
      entry.lastSelfDamageServerSeq === event.damageSeq &&
      entry.lastSelfDamageServerTick === event.serverTick &&
      entry.lastSelfDamageAttackerId === event.shooterId
    );
  });
  if (matching.length < 8) {
    throw new Error(`${label} did not capture enough normal-url post-damage HUD samples: ${JSON.stringify({
      event,
      traceStart,
      matching,
      traceTail: trace.slice(-40),
    })}`);
  }

  const first = matching[0];
  const windowSamples = matching.filter((entry) => entry.at <= first.at + damageSampleWindowMs);
  if (windowSamples.length < 6) {
    throw new Error(`${label} did not keep enough samples inside the manual damage window: ${JSON.stringify({
      event,
      damageSampleWindowMs,
      first,
      windowSamples,
      matchingTail: matching.slice(-40),
    })}`);
  }

  // Low-cadence ordinary-play metrics may miss the instant post-hit display.
  // The persistent last-damage identity/result must match, and live/server
  // energy must never drop below the result while recharge proceeds.
  const invalid = windowSamples.filter((entry) => {
    const energy = sampledEnergy(entry, energyField, remotePlayerId);
    if ((energy ?? Number.NaN) < event.energyAfter - 2) {
      return true;
    }
    if (remotePlayerId == null) {
      return (
        Math.abs((entry.lastSelfDamageEnergy ?? Number.NaN) - event.energyAfter) > 1 ||
        (entry.serverPlayerEnergy ?? Number.NaN) < event.energyAfter - 2
      );
    }
    const remote = sampledRemotePlayer(entry, remotePlayerId);
    return (
      !remote ||
      Math.abs((entry.lastNetworkDamageEnergy ?? Number.NaN) - event.energyAfter) > 1 ||
      (remote.serverEnergy ?? Number.NaN) < event.energyAfter - 2
    );
  });
  if (invalid.length > 0) {
    throw new Error(`${label} live post-damage energy contradicted the authoritative result: ${JSON.stringify({
      event,
      damageSampleWindowMs,
      invalid,
      windowSamples,
    })}`);
  }

  // The bar should recharge upward rather than fall back down.
  const firstWindowEnergy = sampledEnergy(first, energyField, remotePlayerId);
  const values = windowSamples.map((entry) => sampledEnergy(entry, energyField, remotePlayerId));
  const min = Math.min(...values);
  if (min < event.energyAfter - 2) {
    throw new Error(`${label} normal-url live damage display dipped below the authoritative result: ${JSON.stringify({
      event,
      damageSampleWindowMs,
      min,
      windowSamples,
    })}`);
  }
  let previousEnergy = firstWindowEnergy;
  const regressed = windowSamples.slice(1).filter((entry) => {
    const energy = sampledEnergy(entry, energyField, remotePlayerId);
    const dropped = (energy ?? Number.NaN) < previousEnergy - 2;
    previousEnergy = Math.max(previousEnergy, energy ?? previousEnergy);
    return dropped;
  });
  if (regressed.length > 0) {
    throw new Error(`${label} normal-url live damage display regressed instead of recharging upward: ${JSON.stringify({
      event,
      damageSampleWindowMs,
      regressed,
      windowSamples,
    })}`);
  }
}

async function fireManualOneTap({
  label,
  shooter,
  victim,
  server,
  shooterId,
  victimId,
  sinceCount,
}) {
  const visibleFull = await waitForManualVisibleFull(victim, `${label} visible full before shot`);
  await waitUntil(`${label} shooter rendered victim full`, async () => {
    const [source, target] = await Promise.all([shooter.metricsState(), victim.metricsState()]);
    const remote = remotePlayerMetric(source.uiMetrics || {}, victimId);
    const max = target.uiMetrics?.playerMaxEnergy ?? 1700;
    return (remote?.serverEnergy ?? 0) >= max - 1 && (remote?.energy ?? 0) >= max - 1;
  }, 10_000);
  const [shooterBefore, victimBefore] = await Promise.all([
    shooter.metricsState(),
    victim.metricsState(),
  ]);
  const [shooterMetricStart, victimMetricStart] = await Promise.all([
    manualMetricsTraceLength(shooter),
    manualMetricsTraceLength(victim),
  ]);
  assertServerFullWhenHudLooksFull(victimBefore.uiMetrics || {}, label);
  const remoteBefore = remotePlayerMetric(shooterBefore.uiMetrics || {}, victimId);
  const max = victimBefore.uiMetrics?.playerMaxEnergy ?? 1700;
  if ((remoteBefore?.serverEnergy ?? 0) < max - 1 || (remoteBefore?.energy ?? 0) < max - 1) {
    throw new Error(`${label} shooter remote view was not full when victim HUD looked full: ${JSON.stringify({
      visibleFull,
      victimBefore: compactMetrics(victimBefore.uiMetrics),
      shooterBefore: compactMetrics(shooterBefore.uiMetrics),
      remoteBefore,
    })}`);
  }

  await shooter.focus();
  await waitForCombatViewWeaponReady(shooter, label);
  await shooter.press("Control", bulletPressMs, false);
  await waitUntil(`${label} one server damage event`, async () => {
    assertNoBulletSelfDamage(server, sinceCount, label);
    return matchingEvents(server, sinceCount, shooterId, victimId).length >= 1;
  }, 10_000);

  assertNoBulletSelfDamage(server, sinceCount, label);
  const events = matchingEvents(server, sinceCount, shooterId, victimId);
  if (events.length !== 1) {
    throw new Error(`${label} expected exactly one server damage event: ${JSON.stringify({
      events,
      serverDamageEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
    })}`);
  }
  const [event] = events;
  assertServerDamageAccounting(event, label);
  if (event.damage !== 400 || event.energyBefore !== 1700 || event.energyAfter !== 1300) {
    throw new Error(`${label} manual one-tap bullet was not exactly 1700 -> 1300: ${JSON.stringify({
      event,
      visibleFull,
      victimBefore: compactMetrics(victimBefore.uiMetrics),
      shooterBefore: compactMetrics(shooterBefore.uiMetrics),
      remoteBefore,
    })}`);
  }

  const damageHud = await waitForManualDamageHud({ victim, shooter, event, label });
  await Promise.all([
    assertManualDamageWindow({
      client: victim,
      event,
      label: `${label} victim HUD`,
      traceStart: victimMetricStart,
      energyField: "playerEnergy",
    }),
    assertManualDamageWindow({
      client: shooter,
      event,
      label: `${label} shooter remote HUD`,
      traceStart: shooterMetricStart,
      energyField: "remoteEnergy",
      remotePlayerId: victimId,
    }),
  ]);
  return {
    event,
    visibleFull,
    damageHud,
    nextSinceCount: parseServerDamageEvents(server.logs).length,
  };
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  if (!Number.isFinite(bulletPressMs) || bulletPressMs <= 0) {
    throw new Error(`invalid ZUBSPACE_E2E_MANUAL_BULLET_PRESS_MS=${process.env.ZUBSPACE_E2E_MANUAL_BULLET_PRESS_MS || ""}`);
  }
  if (!Number.isInteger(fullRegenRepeats) || fullRegenRepeats < 1) {
    throw new Error(`invalid ZUBSPACE_E2E_MANUAL_FULL_REGEN_REPEATS=${process.env.ZUBSPACE_E2E_MANUAL_FULL_REGEN_REPEATS || ""}`);
  }

  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/manual_bullet_damage.mjs" });
  const serverPort = await freePort();
  const alphaPort = await freePort();
  const betaPort = await freePort();
  const staticServer = await startStaticServer();
  // This one-tap oracle deliberately starts both stationary ships together;
  // independent production random spawns do not establish a firing lane.
  const spawnFixture = arena === "chaos-zone" ? createSpawnRegionFixture({
    zoneId: arena, spawnRegion: { x: 512, y: 512, radius: 0 },
  }) : null;
  const server = startRealServer(serverPort, spawnFixture ? {
    zoneRoot: spawnFixture.root, env: { ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" },
  } : {});
  const alphaChrome = startChrome(alphaPort, "manual-alpha");
  const betaChrome = startChrome(betaPort, "manual-beta");
  let alpha = null;
  let beta = null;
  let alphaUrl = null;
  let betaUrl = null;
  const capture = () => ({
    server, serverPort, staticServer, urls: [alphaUrl, betaUrl],
    clients: [{ label: "alpha", client: alpha }, { label: "beta", client: beta }],
    extraLogs: { "alpha-chrome": alphaChrome.logs, "beta-chrome": betaChrome.logs,
      ...(spawnFixture ? {
        "single-bullet-spawn-fixture": JSON.stringify(spawnFixture),
        "single-bullet-scenario-settings": fs.readFileSync(path.join(spawnFixture.root, arena, "arena.conf"), "utf8"),
        "single-bullet-zone-bundle": fs.readFileSync(path.join(spawnFixture.root, arena, "zone.bundle"), "utf8"),
      } : {}),
    },
    // The delivery audit asserts an ingress drain, which is disconnect-cleanup evidence,
    // so it closes both sessions itself after the live-page artifact capture. Each client
    // is guarded so one already-dead session cannot make cleanup throw.
    disconnect: async () => {
      await Promise.all([
        alpha?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
        beta?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
      ]);
    },
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await Promise.all([waitForChrome(alphaChrome), waitForChrome(betaChrome)]);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&password=secret&arena=${encodeURIComponent(arena)}&metrics=1`;
    alphaUrl = `${pageBase}&name=${encodeURIComponent(playerNames.alpha)}`;
    betaUrl = `${pageBase}&name=${encodeURIComponent(playerNames.beta)}`;
    [alpha, beta] = await Promise.all([
      openChromeClient(alphaChrome, alphaUrl),
      openChromeClient(betaChrome, betaUrl),
    ]);
    await Promise.all([installManualMetricsSampler(alpha), installManualMetricsSampler(beta)]);
    await Promise.all([alpha.enterShip(), beta.enterShip()]);
    await waitForManualPlayersReady(alpha, beta);

    const [alphaStart, betaStart] = await Promise.all([alpha.metricsState(), beta.metricsState()]);
    const alphaId = alphaStart.uiMetrics?.selfPlayerId;
    const betaId = betaStart.uiMetrics?.selfPlayerId;
    if (alphaId == null || betaId == null || alphaId === betaId) {
      throw new Error(`missing distinct player ids: ${JSON.stringify({ alphaId, betaId })}`);
    }
    if (spawnFixture) {
      const positions = [alphaStart.uiMetrics.playerX, alphaStart.uiMetrics.playerY,
        betaStart.uiMetrics.playerX, betaStart.uiMetrics.playerY];
      if (!positions.every(Number.isFinite) || Math.hypot(positions[0] - positions[2], positions[1] - positions[3]) > 4) {
        throw new Error("manual one-tap fixture did not establish its collocated firing lane");
      }
    }

    let sinceCount = parseServerDamageEvents(server.logs).length;
    const events = [];
    const alphaFirst = await fireManualOneTap({
      label: "manual normal-url alpha first shot to beta",
      shooter: alpha,
      victim: beta,
      server,
      shooterId: alphaId,
      victimId: betaId,
      sinceCount,
    });
    events.push(alphaFirst.event);
    sinceCount = alphaFirst.nextSinceCount;

    const betaImmediateCounter = await fireManualOneTap({
      label: "manual normal-url damaged beta immediate counter shot to alpha",
      shooter: beta,
      victim: alpha,
      server,
      shooterId: betaId,
      victimId: alphaId,
      sinceCount,
    });
    events.push(betaImmediateCounter.event);
    sinceCount = betaImmediateCounter.nextSinceCount;

    for (let repeat = 1; repeat <= fullRegenRepeats; repeat += 1) {
      const alphaRepeat = await fireManualOneTap({
        label: `manual normal-url alpha full-regen shot ${repeat + 1} to beta`,
        shooter: alpha,
        victim: beta,
        server,
        shooterId: alphaId,
        victimId: betaId,
        sinceCount,
      });
      events.push(alphaRepeat.event);
      sinceCount = alphaRepeat.nextSinceCount;

      const betaRepeat = await fireManualOneTap({
        label: `manual normal-url beta full-regen shot ${repeat + 1} to alpha`,
        shooter: beta,
        victim: alpha,
        server,
        shooterId: betaId,
        victimId: alphaId,
        sinceCount,
      });
      events.push(betaRepeat.event);
      sinceCount = betaRepeat.nextSinceCount;
    }

    const [alphaAfter, betaAfter] = await Promise.all([alpha.metricsState(), beta.metricsState()]);
    const result = await evidence.succeed({
      ok: true,
      transport: "rtc",
      browserPair: "chrome-chrome",
      arena,
      traceUrl: false,
      packetLogUrl: false,
      bulletPressMs,
      fullRegenRepeats,
      primaryControl: "Control",
      spawnFixture,
      playerName,
      playerNames,
      alphaId,
      betaId,
      events,
      alpha: compactMetrics(alphaAfter.uiMetrics),
      beta: compactMetrics(betaAfter.uiMetrics),
    }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    throw new Error(`${error.message}\n${browserE2eFailureFooter(artifacts)}`, { cause: error });
  } finally {
    await alpha?.close().catch(() => {});
    await beta?.close().catch(() => {});
    await Promise.all([
      stopProcess(alphaChrome.child),
      stopProcess(betaChrome.child),
      stopProcess(server.child),
    ]).catch(() => {});
    fs.rmSync(alphaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    fs.rmSync(betaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
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
