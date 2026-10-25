#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  assertServerDamageAccounting,
  assertSingleBrowserDamageTrace,
  browserE2eFailureFooter,
  compactMetrics,
  openChromeClient,
  parseServerDamageEvents,
  startChrome,
  summarizeClient,
  waitForChrome,
  waitForPlayersReady,
  rendezvousPlayers,
  waitUntil,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";

import {
  createSession,
  deleteSession,
  execute as executeFirefox,
  firefoxArtifactClient,
  freePort,
  metricsState as firefoxMetricsState,
  openClient as openFirefoxClient,
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
import { parseZubspaceEvents } from "../../tools/server_events.mjs";

const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const bulletPressMs = Number(process.env.ZUBSPACE_E2E_SINGLE_BULLET_PRESS_MS || "30");
const damageSampleWindowMs = Number(process.env.ZUBSPACE_E2E_DAMAGE_WINDOW_MS || "3000");
const playerName = process.env.ZUBSPACE_E2E_MIXED_PLAYER_NAME || "mixedpilot";
// The server deliberately rejects duplicate handles. Keep both real clients
// distinct within the14-character limit, including a caller-provided prefix.
const playerNames = { alpha: `${playerName.slice(0, 13)}1`, beta: `${playerName.slice(0, 13)}2` };
const serverTicksPerSecond = 100;
const presentationRecoveryTicksPerSecond = 200;

function matchingEvents(server, sinceCount, shooterId, victimId) {
  return parseServerDamageEvents(server.logs)
    .slice(sinceCount)
    .filter((event) => event.shooterId === shooterId && event.victimId === victimId);
}

function assertPresentationGeometryWitness(server, damageEvent, label) {
  const witnesses = parseZubspaceEvents(server.logs)
    .map(({ event }) => event)
    .filter((event) =>
      event.type === "server_collision_witness" &&
      event.projectile_id === damageEvent.projectileId &&
      event.owner_id === damageEvent.shooterId &&
      event.owner_life_seq === damageEvent.shooterLifeSeq &&
      event.target_id === damageEvent.victimId &&
      event.target_life_seq === damageEvent.victimLifeSeq
    );
  if (witnesses.length !== 1) {
    throw new Error(`${label} expected one exact collision witness: ${JSON.stringify({
      damageEvent,
      witnesses,
    })}`);
  }

  const geometry = witnesses[0].presentation_geometry;
  const resolved = typeof geometry?.status === "string" && geometry.status.startsWith("resolved_");
  if (
    !resolved ||
    witnesses[0].history_source !== "observer_presentation" ||
    !Number.isInteger(geometry.accepted_timeline_generation) ||
    geometry.accepted_timeline_generation <= 0 ||
    geometry.effective_timeline_generation !== geometry.accepted_timeline_generation ||
    !Number.isInteger(geometry.render_tick) ||
    witnesses[0].requested_target_tick !== geometry.render_tick ||
    witnesses[0].selected_target_tick !== geometry.render_tick ||
    !Number.isInteger(geometry.applied_snapshot_id) ||
    geometry.applied_snapshot_id <= 0 ||
    !Number.isInteger(geometry.lower_snapshot_id) ||
    geometry.lower_snapshot_id <= 0 ||
    !Number.isInteger(geometry.upper_snapshot_id) ||
    geometry.upper_snapshot_id <= 0 ||
    !Number.isFinite(geometry.target_x) ||
    !Number.isFinite(geometry.target_y) ||
    witnesses[0].collision_anchor_x !== geometry.target_x ||
    witnesses[0].collision_anchor_y !== geometry.target_y ||
    !Number.isFinite(geometry.authority_x) ||
    !Number.isFinite(geometry.authority_y) ||
    !Number.isFinite(geometry.authority_error) ||
    geometry.authority_error > 12
  ) {
    throw new Error(`${label} has no authoritative combat-timeline collision geometry: ${JSON.stringify({
      damageEvent,
      witness: witnesses[0],
    })}`);
  }
  return witnesses[0];
}

function forwardTickDistance(target, current) {
  if (!Number.isInteger(target) || !Number.isInteger(current)) return null;
  const distance = (target - current) >>> 0;
  return distance < 0x8000_0000 ? distance : null;
}

async function waitForCombatViewAfterRetainedCapture(client, label, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  let lastMetrics = null;
  while (Date.now() < deadline) {
    const state = await client.combatState();
    const metrics = state.uiMetrics || {};
    const timeline = metrics.combatViewTimeline || {};
    lastMetrics = metrics;
    if (
      (metrics.playerRespawnTimer ?? 0) <= 0 &&
      (metrics.playerEnergy ?? 0) > 0 &&
      timeline.weaponReady === true
    ) {
      return;
    }

    // A retained Firefox state capture is intentionally comprehensive and can
    // synchronously pause the debug tab for several seconds. V1 advances the
    // drawn cursor at 200 Hz toward a server schedule moving at 100 Hz. Leave
    // the tab unperturbed for the exact minimum catch-up interval implied by
    // the observed serial tick gap, then inspect it again. Repeated WebDriver
    // polling here would itself extend the scheduler stall being measured.
    const gap = forwardTickDistance(
      timeline.scheduledRenderTick,
      timeline.presentationRenderTick,
    );
    const netRecoveryTicksPerSecond =
      presentationRecoveryTicksPerSecond - serverTicksPerSecond;
    const catchUpMs = gap == null
      ? 100
      : Math.max(100, Math.ceil((gap * 1000) / netRecoveryTicksPerSecond));
    const remainingMs = deadline - Date.now();
    if (remainingMs <= 0) break;
    await sleep(Math.min(catchUpMs, remainingMs));
  }
  throw new Error(
    `timed out waiting for ${label} shooter combat view ready after retained-state capture: ${JSON.stringify(lastMetrics)}`,
  );
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

async function assertShooterDidNotSelfDie(client, beforeState, label) {
  const afterState = await client.metricsState();
  const beforeMetrics = beforeState.uiMetrics || {};
  const afterMetrics = afterState.uiMetrics || {};
  const beforeRespawnSeq = beforeMetrics.playerRespawnSeq ?? 0;
  const beforeLosses = beforeMetrics.selfLosses ?? 0;
  const selfId = afterMetrics.selfPlayerId ?? beforeMetrics.selfPlayerId;
  const newBulletSelfDamage = (afterState.damageTrace || [])
    .slice(beforeState.damageTrace?.length ?? 0)
    .filter((entry) =>
      entry.forSelf === true &&
      entry.victimId === selfId &&
      entry.attackerId === selfId &&
      [1, 2, 7, 15].includes(entry.weapon & 0x001f)
    );

  if (
    afterMetrics.playerAlive !== true ||
    afterMetrics.serverPlayerAlive !== true ||
    (afterMetrics.playerRespawnTimer ?? 0) > 0 ||
    (afterMetrics.playerRespawnSeq ?? 0) > beforeRespawnSeq ||
    (afterMetrics.selfLosses ?? 0) > beforeLosses ||
    newBulletSelfDamage.length > 0
  ) {
    throw new Error(`${label} shooter died or recorded bullet self-damage while firing: ${JSON.stringify({
      before: beforeMetrics,
      after: afterMetrics,
      newBulletSelfDamage,
      damageTrace: (afterState.damageTrace || []).slice(beforeState.damageTrace?.length ?? 0),
    })}`);
  }
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
  switch (key) {
    case "Control":
      return "\uE009";
    case "Space":
      return "\uE00D";
    case "ArrowLeft":
      return "\uE012";
    case "ArrowUp":
      return "\uE013";
    case "ArrowRight":
      return "\uE014";
    case "ArrowDown":
      return "\uE015";
    default:
      return key;
  }
}

async function performFirefoxKeyActions(driver, actions) {
  await webdriver(driver, "POST", `/session/${driver.sessionId}/actions`, {
    actions: [
      {
        type: "key",
        id: "keyboard",
        actions,
      },
    ],
  });
  await webdriver(driver, "DELETE", `/session/${driver.sessionId}/actions`);
}

async function openFirefoxCombatClient(driver, url) {
  await openFirefoxClient(driver, url);
  return {
    label: driver.label,
    async focus() {
      await executeFirefox(driver, "document.querySelector('canvas')?.focus(); return true;");
    },
    async press(key, holdMs = 80, focusBefore = true) {
      if (focusBefore) await this.focus();
      const value = firefoxKeyValue(key);
      await performFirefoxKeyActions(driver, [
        { type: "keyDown", value },
        { type: "pause", duration: holdMs },
        { type: "keyUp", value },
      ]);
    },
    async enterShip() {
      await this.focus();
      await waitUntil(`${driver.label} ordinary join playable`, async () => {
        const state = await this.metricsState();
        return state.uiMetrics?.localGameplayState?.authorityPhase === "Live";
      }, 20_000);
    },
    ...firefoxArtifactClient(driver),
    screenshot: () => screenshot(driver),
    async metricsState() {
      return firefoxMetricsState(driver);
    },
    async combatState() {
      return executeFirefox(
        driver,
        `const metrics = window.__zubspaceUiMetrics || {};
         return {
           uiMetrics: {
             selfPlayerId: metrics.selfPlayerId,
             playerLifeSeq: metrics.playerLifeSeq,
             clientInputSeq: metrics.clientInputSeq,
             playerRespawnSeq: metrics.playerRespawnSeq,
             playerRespawnTimer: metrics.playerRespawnTimer,
             playerEnergy: metrics.playerEnergy,
             playerAlive: metrics.playerAlive,
             serverPlayerAlive: metrics.serverPlayerAlive,
             selfLosses: metrics.selfLosses,
             combatViewTimeline: metrics.combatViewTimeline || null,
           },
           damageTrace: (window.__zubspaceDamageTrace || []).slice(-256),
           deathTrace: (window.__zubspaceDeathTrace || []).slice(-64),
         };`,
      );
    },
    async close() {
      await deleteSession(driver);
    },
  };
}

async function fireOneBulletAndAssert({
  label,
  shooter,
  victim,
  server,
  shooterId,
  victimId,
  sinceCount,
}) {
  const [shooterBefore, victimBefore] = await Promise.all([
    shooter.combatState(),
    victim.combatState(),
  ]);
  const shooterTraceStart = shooterBefore.damageTrace?.length ?? 0;
  const victimTraceStart = victimBefore.damageTrace?.length ?? 0;

  // Focus first because a separate WebDriver script after the readiness read
  // can itself cross multiple browser frames. Keep the per-shot boundary
  // compact. Serializing complete Firefox packet,
  // sprite, and metrics histories here created the very scheduler hold this
  // test then tried to recover from. The bounded damage/death tails and exact
  // lifecycle fields preserve every assertion without perturbing the browser.
  // Still require the explicit drawn/current boundary before the one genuine
  // CDP/WebDriver key press; failure to recover remains fatal.
  await shooter.focus();
  await waitForCombatViewAfterRetainedCapture(shooter, label);

  await shooter.press("Control", bulletPressMs, false);
  await waitUntil(`${label} server damage`, async () => {
    assertNoBulletSelfDamage(server, sinceCount, label);
    return matchingEvents(server, sinceCount, shooterId, victimId).length >= 1;
  }, 10_000);

  assertNoBulletSelfDamage(server, sinceCount, label);
  const events = matchingEvents(server, sinceCount, shooterId, victimId);
  if (events.length !== 1) {
    throw new Error(`${label} expected one authoritative bullet hit: ${JSON.stringify({
      events,
      serverDamageEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
    })}`);
  }

  const [event] = events;
  assertServerDamageAccounting(event, label);
  if (event.damage !== 400 || event.energyBefore !== 1700 || event.energyAfter !== 1300) {
    throw new Error(`${label} unexpected damage accounting: ${JSON.stringify(event)}`);
  }
  assertPresentationGeometryWitness(server, event, label);

  await Promise.all([
    assertVisibleDamageWindow(victim, event, `${label} victim local HUD`, {
      energyField: "playerEnergy",
      seqField: "lastSelfDamageServerSeq",
      requireLocalDetails: true,
    }),
    assertVisibleDamageWindow(shooter, event, `${label} shooter remote HUD`, {
      energyField: "remoteEnergy",
      seqField: "lastNetworkDamageServerSeq",
      requireLocalDetails: false,
      remoteVictimId: event.victimId,
    }),
  ]);
  await Promise.all([
    assertSingleBrowserDamageTrace(shooter, event, `${label} shooter`, shooterTraceStart),
    assertSingleBrowserDamageTrace(victim, event, `${label} victim`, victimTraceStart),
  ]);
  await assertShooterDidNotSelfDie(shooter, shooterBefore, label);

  return {
    event,
    nextSinceCount: parseServerDamageEvents(server.logs).length,
  };
}

async function assertVisibleDamageWindow(client, event, label, {
  energyField,
  seqField,
  requireLocalDetails,
  remoteVictimId = null,
}) {
  const remoteVictim = (metrics) =>
    remoteVictimId == null
      ? null
      : (metrics.remotePlayers || []).find((player) => player.id === remoteVictimId) || null;
  const samples = [];
  let firstMetrics = null;
  await waitUntil(`${label} received damage metrics`, async () => {
    const state = await client.metricsState();
    const metrics = state.uiMetrics || {};
    const remote = remoteVictim(metrics);
    const damageEnergy = remoteVictimId == null
      ? metrics.lastSelfDamageEnergy
      : metrics.lastNetworkDamageEnergy;
    if (
      metrics[seqField] === event.damageSeq &&
      Number.isFinite(metrics[energyField]) &&
      Math.abs((damageEnergy ?? Number.NaN) - event.energyAfter) <= 1 &&
      (remoteVictimId == null || remote)
    ) {
      firstMetrics = metrics;
      return true;
    }
    return false;
  }, 8_000);

  const startedAt = Date.now();
  while (Date.now() - startedAt <= damageSampleWindowMs) {
    const state = await client.metricsState();
    const metrics = state.uiMetrics || {};
    if (metrics[seqField] === event.damageSeq && Number.isFinite(metrics[energyField])) {
      const remote = remoteVictim(metrics);
      samples.push({
        at: Date.now(),
        [energyField]: metrics[energyField],
        seq: metrics[seqField],
        damageEnergy: remoteVictimId == null
          ? metrics.lastSelfDamageEnergy
          : metrics.lastNetworkDamageEnergy,
        serverEnergy: remoteVictimId == null
          ? metrics.serverPlayerEnergy
          : remote?.serverEnergy ?? null,
        localShipLabelEnergy: metrics.localShipLabelEnergy,
        visibleEnergyDigits: metrics.visibleEnergyDigits,
      });
    }
    await sleep(100);
  }

  if (samples.length < 2) {
    throw new Error(`${label} did not capture enough live post-damage samples: ${JSON.stringify({
      event,
      firstMetrics,
      samples,
    })}`);
  }

  // Energy display is now fully live: after a hit the bar shows the post-hit
  // value for an instant (delivered by the WATCH_DAMAGE event) and then
  // recharges immediately, following the live server energy. The live displayed
  // field therefore only equals `energyAfter` for an instant and a sampled poll
  // may already have recharged past it. Key the hit off persistent server-owned
  // damage identity/result metrics and ensure live/server energy never drops
  // below that result while recharge advances.
  const invalid = samples.filter((entry) =>
    entry[energyField] < event.energyAfter - 2 ||
    Math.abs((entry.damageEnergy ?? Number.NaN) - event.energyAfter) > 1 ||
    (entry.serverEnergy ?? Number.NaN) < event.energyAfter - 2
  );
  if (invalid.length > 0) {
    throw new Error(`${label} live post-damage energy contradicted the authoritative damage result: ${JSON.stringify({
      event,
      firstMetrics,
      damageSampleWindowMs,
      invalid,
      samples,
    })}`);
  }

  // First sample's live displayed channels must not show over-damage either.
  const firstSample = samples[0];
  if (
    requireLocalDetails &&
    (
      (firstSample.localShipLabelEnergy ?? Number.NaN) < event.energyAfter - 2 ||
      (firstSample.visibleEnergyDigits ?? Number.NaN) < event.energyAfter - 2
    )
  ) {
    throw new Error(`${label} first visible post-damage sample displayed below the authoritative damage result: ${JSON.stringify({
      event,
      firstMetrics,
      firstSample,
      samples,
    })}`);
  }

  // The bar should recharge upward rather than fall back down.
  let previousEnergy = firstSample[energyField];
  const regressed = samples.slice(1).filter((entry) => {
    const energy = entry[energyField];
    const dropped = energy < previousEnergy - 2;
    previousEnergy = Math.max(previousEnergy, energy);
    return dropped;
  });
  if (regressed.length > 0) {
    throw new Error(`${label} live post-damage energy regressed instead of recharging upward: ${JSON.stringify({
      event,
      firstMetrics,
      damageSampleWindowMs,
      regressed,
      samples,
    })}`);
  }
}

async function waitForVictimEnergyFull(shooter, victim, label) {
  let full = null;
  await waitUntil(`${label} victim full locally, remotely, and on server`, async () => {
    const [shooterState, victimState] = await Promise.all([
      shooter.metricsState(),
      victim.metricsState(),
    ]);
    const metrics = victimState.uiMetrics || {};
    const max = metrics.playerMaxEnergy ?? 1700;
    const remoteEnergy = shooterState.uiMetrics?.remoteEnergy ?? 0;
    if (
      metrics.playerAlive === true &&
      metrics.serverPlayerAlive === true &&
      (metrics.playerEnergy ?? 0) >= max - 1 &&
      (metrics.visibleEnergyDigits ?? 0) >= max - 1 &&
      (metrics.serverPlayerEnergy ?? 0) >= max - 1 &&
      remoteEnergy >= max - 1
    ) {
      full = { victim: metrics, shooterRemote: shooterState.uiMetrics };
      return true;
    }
    return false;
  }, 18_000);
  return full;
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }

  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/mixed_single_bullet_damage.mjs" });
  const serverPort = await freePort();
  const chromePort = await freePort();
  const geckoPort = await freePort();
  const staticServer = await startStaticServer();
  // The measured fixture requires collocated stationary ships. Declare that
  // geometry through the real server instead of assuming random spawns match.
  const spawnFixture = arena === "chaos-zone" ? createSpawnRegionFixture({
    zoneId: arena, spawnRegion: { x: 512, y: 512, radius: 0 },
  }) : null;
  const server = startRealServer(serverPort, spawnFixture ? {
    zoneRoot: spawnFixture.root, env: { ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" },
  } : {});
  const chromeProcess = startChrome(chromePort, "mixed-chrome");
  const firefoxDriver = startGeckodriver(geckoPort, "mixed-firefox");
  let chrome = null;
  let firefox = null;
  let chromeUrl = null;
  let firefoxUrl = null;
  let bootstrapResults = [];
  const firefoxBootstrapCapture = {
    ...firefoxArtifactClient(firefoxDriver),
    screenshot: () => screenshot(firefoxDriver),
  };
  const capture = () => ({
    server, serverPort, staticServer, urls: [chromeUrl, firefoxUrl],
    clients: [{ label: "chrome", client: chrome },
      { label: "firefox", client: firefox || firefoxBootstrapCapture }],
    extraLogs: { chrome: chromeProcess.logs, geckodriver: firefoxDriver.logs,
      bootstrap: JSON.stringify(bootstrapResults, null, 2),
      ...(spawnFixture ? {
        "single-bullet-spawn-fixture": JSON.stringify(spawnFixture),
        "single-bullet-scenario-settings": fs.readFileSync(path.join(spawnFixture.root, arena, "arena.conf"), "utf8"),
        "single-bullet-zone-bundle": fs.readFileSync(path.join(spawnFixture.root, arena, "zone.bundle"), "utf8"),
      } : {}),
    },
    // The delivery audit asserts an ingress drain, which is disconnect-cleanup evidence,
    // so it closes both sessions itself after the live-page artifact capture. The
    // parenthesized comma form is one expression, so both the Chrome DevTools evaluator and
    // the Firefox `return (...)` script wrapper accept it; each session is guarded so one
    // already-dead session cannot make cleanup throw.
    disconnect: async () => {
      await Promise.all([
        chrome?.evaluate("(window.__zubspaceSendDisconnectForTest?.(), true)").catch(() => {}),
        firefox?.evaluate("(window.__zubspaceSendDisconnectForTest?.(), true)").catch(() => {}),
      ]);
    },
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await Promise.all([waitForChrome(chromeProcess), waitForGeckodriver(firefoxDriver)]);
    await createSession(firefoxDriver);

    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1`;
    chromeUrl = `${pageBase}&name=${encodeURIComponent(playerNames.alpha)}`;
    firefoxUrl = `${pageBase}&name=${encodeURIComponent(playerNames.beta)}`;
    // Preserve each successful handle independently. A failed peer must not
    // erase the other peer or prevent capture of its still-live Firefox page.
    const opening = await Promise.allSettled([
      openChromeClient(chromeProcess, chromeUrl).then(client => (chrome = client)),
      openFirefoxCombatClient(firefoxDriver, firefoxUrl).then(client => (firefox = client)),
    ]);
    bootstrapResults = opening.map((result, index) => ({
      browser: index === 0 ? "chrome" : "firefox", status: result.status,
      error: result.status === "rejected" ? String(result.reason?.stack || result.reason) : null,
    }));
    const failedOpen = opening.find(result => result.status === "rejected");
    if (failedOpen) throw failedOpen.reason;

    await Promise.all([chrome.enterShip(), firefox.enterShip()]);
    if (!spawnFixture) await rendezvousPlayers(chrome, firefox, server);
    await waitForPlayersReady(chrome, firefox);

    const [chromeBefore, firefoxBefore] = await Promise.all([
      chrome.pageState(),
      firefox.pageState(),
    ]);
    const chromeId = summarizeClient(chromeBefore).selfId;
    const firefoxId = summarizeClient(firefoxBefore).selfId;
    if (chromeId == null || firefoxId == null || chromeId === firefoxId) {
      throw new Error(`missing distinct player ids: ${JSON.stringify({ chromeId, firefoxId })}`);
    }

    const initial = await Promise.all([chrome.metricsState(), firefox.metricsState()]);
    for (const state of initial) {
      const m = state.uiMetrics || {};
      if (Math.hypot(m.playerVelocityX ?? Infinity, m.playerVelocityY ?? Infinity) > 1 ||
          Object.values(m.semanticInputCounts || {}).some(count => count !== 0)) {
        throw new Error(`ordinary join generated unintended movement/action: ${JSON.stringify(m)}`);
      }
    }
    const [first, second] = initial.map(state => state.uiMetrics);
    if (Math.hypot(first.playerX - second.playerX, first.playerY - second.playerY) > 4) {
      throw new Error("single-bullet initial fixture is not collocated");
    }

    let sinceCount = parseServerDamageEvents(server.logs).length;
    const events = [];
    const chromeToFirefoxFirst = await fireOneBulletAndAssert({
      label: "mixed chrome to firefox first shot",
      shooter: chrome,
      victim: firefox,
      server,
      shooterId: chromeId,
      victimId: firefoxId,
      sinceCount,
    });
    events.push(chromeToFirefoxFirst.event);
    sinceCount = chromeToFirefoxFirst.nextSinceCount;

    await waitForVictimEnergyFull(chrome, firefox, "mixed chrome to firefox regen before second shot");
    const chromeToFirefoxSecond = await fireOneBulletAndAssert({
      label: "mixed chrome to firefox second shot after full regen",
      shooter: chrome,
      victim: firefox,
      server,
      shooterId: chromeId,
      victimId: firefoxId,
      sinceCount,
    });
    events.push(chromeToFirefoxSecond.event);
    sinceCount = chromeToFirefoxSecond.nextSinceCount;

    await waitForVictimEnergyFull(chrome, firefox, "mixed firefox shooter regen before counterfire");
    const firefoxToChromeFirst = await fireOneBulletAndAssert({
      label: "mixed firefox to chrome first counter shot",
      shooter: firefox,
      victim: chrome,
      server,
      shooterId: firefoxId,
      victimId: chromeId,
      sinceCount,
    });
    events.push(firefoxToChromeFirst.event);
    sinceCount = firefoxToChromeFirst.nextSinceCount;

    await waitForVictimEnergyFull(firefox, chrome, "mixed firefox to chrome regen before second counter shot");
    const firefoxToChromeSecond = await fireOneBulletAndAssert({
      label: "mixed firefox to chrome second shot after full regen",
      shooter: firefox,
      victim: chrome,
      server,
      shooterId: firefoxId,
      victimId: chromeId,
      sinceCount,
    });
    events.push(firefoxToChromeSecond.event);
    sinceCount = firefoxToChromeSecond.nextSinceCount;

    assertNoBulletSelfDamage(server, 0, "mixed single-bullet run");
    const [chromeAfter, firefoxAfter] = await Promise.all([
      chrome.pageState(),
      firefox.pageState(),
    ]);
    const result = await evidence.succeed({
      ok: true,
      transport: "rtc",
      browserPair: "chrome-firefox",
      arena,
      bulletPressMs,
      spawnFixture,
      playerName,
      playerNames,
      chromeId,
      firefoxId,
      events,
      chrome: compactMetrics(chromeAfter.uiMetrics),
      firefox: compactMetrics(firefoxAfter.uiMetrics),
    }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    throw new Error(`${error.message}\n${browserE2eFailureFooter(artifacts)}`, { cause: error });
  } finally {
    await chrome?.close().catch(() => {});
    await firefox?.close().catch(() => {});
    await Promise.all([
      stopProcess(chromeProcess.child),
      stopProcess(firefoxDriver.child),
      stopProcess(server.child),
    ]).catch(() => {});
    fs.rmSync(chromeProcess.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
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
