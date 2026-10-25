#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

import {
  execute as executeFirefox,
  createSession, deleteSession, enterShip as firefoxEnterShip, firefoxArtifactClient,
  firefoxKeyDown, firefoxKeyUp, holdFirefoxKeys, metricsState as firefoxMetricsState, openClient as openFirefoxClient,
  screenshot as firefoxScreenshot, startGeckodriver, waitForGeckodriver,
  freePort,
  repoRoot,
  sleep,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {
  browserE2eFailureFooter,
  openChromeClient,
  summarizeClient,
  startChrome,
  waitForChrome,
  waitForPlayersReady, rendezvousPlayers,
  waitForCombatViewWeaponReady,
  waitUntil,
} from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

// Real-key spectator admission, free camera, whole-roster follow, and ship re-entry.
// Camera updates must remain independent of the closed gameplay authority gate.

const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const wasmPath = process.env.ZUBSPACE_E2E_STATIC_ROOT
  ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm"))
  : "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const e2eScript = "tests/e2e/spectator.mjs";
async function openFirefoxSpectator(driver, url) {
  await openFirefoxClient(driver, url);
  return {
    ...firefoxArtifactClient(driver),
    evaluate: expression => executeFirefox(driver, `return (0, eval)(${JSON.stringify(expression)});`),
    metricsState: () => firefoxMetricsState(driver),
    keyDown: key => firefoxKeyDown(driver, key),
    keyUp: key => firefoxKeyUp(driver, key),
    press: (key, duration = 70) => holdFirefoxKeys(driver, [{ Escape: "\uE00C", Tab: "\uE004", Enter: "\uE006" }[key] || key], duration),
    enterShip: () => firefoxEnterShip(driver),
    screenshot: () => firefoxScreenshot(driver),
    close: () => deleteSession(driver),
  };
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; rebuild the selected client bundle first`);
  }
  const evidence = createFocusedBrowserEvidence({ e2eScript, wasm: wasmPath,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  const screenshots = {};

  const serverPort = await freePort();
  const alphaPort = await freePort();
  const betaPort = await freePort();
  const staticServer = await startStaticServer();
  // Camera/role coverage needs moving bot targets without a weapon hit racing
  // the required full-energy Esc-S transition. Hostile combat has its own lanes.
  const server = startRealServer(serverPort, { botCount: 2,
    env: { ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ: "1" } });
  const firefox = process.env.ZUBSPACE_E2E_SPECTATOR_BROWSER === "firefox";
  const alphaChrome = firefox ? startGeckodriver(alphaPort, "spectator-firefox") : startChrome(alphaPort, "spectator-alpha");
  const betaChrome = startChrome(betaPort, "spectator-beta");
  let alpha = null;
  let beta = null;
  let alphaUrl = null;
  let betaUrl = null;
  let failedAudioProbe = null;
  const capture = () => ({
    server,
    serverPort,
    staticServer,
    urls: [alphaUrl, betaUrl],
    clients: [
      { label: "alpha", client: alpha },
      { label: "beta", client: beta },
    ],
    extraLogs: { "alpha-browser": alphaChrome.logs, "beta-chrome": betaChrome.logs,
      "spectator-audio-probe": JSON.stringify(failedAudioProbe) },
    extraScreenshots: screenshots,
    disconnect: async () => {
      await Promise.all([
        alpha?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
        beta?.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
      ]);
    },
  });

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await Promise.all([firefox ? waitForGeckodriver(alphaChrome) : waitForChrome(alphaChrome), waitForChrome(betaChrome)]);
    if (firefox) await createSession(alphaChrome);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase =
      `http://127.0.0.1:${staticServer.port}/index.html` +
      `?rtc=${encodeURIComponent(rtcUrl)}` +
      `&password=secret&arena=${encodeURIComponent(arena)}&trace=1&packetlog=1`;
    alphaUrl = `${pageBase}&name=spectatoralpha`;
    betaUrl = `${pageBase}&name=spectatorbeta`;
    [alpha, beta] = await Promise.all([
      firefox ? openFirefoxSpectator(alphaChrome, alphaUrl) : openChromeClient(alphaChrome, alphaUrl),
      openChromeClient(betaChrome, betaUrl),
    ]);
    await Promise.all([alpha.enterShip(), beta.enterShip()]);
    await rendezvousPlayers(alpha, beta, server);
    await waitForPlayersReady(alpha, beta, server);

    const betaId = (await beta.metricsState()).uiMetrics.selfPlayerId;
    const state = async () => (await alpha.metricsState()).uiMetrics;
    await alpha.evaluate(`(() => {
      const proto = window.AudioBufferSourceNode?.prototype;
      if (!proto) throw new Error("game audio source API unavailable");
      window.__zubspaceSpectatorAudio = [];
      const start = proto.start, connect = proto.connect;
      proto.connect = function(destination, ...args) {
        this.__zubspaceObservedGain = destination?.gain;
        return connect.call(this, destination, ...args);
      };
      proto.start = function(...args) {
        const result = start.apply(this, args);
        const rows = window.__zubspaceSpectatorAudio;
        rows.push({ at: Date.now(), running: this.context.state === "running",
          volume: this.__zubspaceObservedGain?.value ?? 1, duration: this.buffer?.duration ?? 0 });
        if (rows.length > 64) rows.shift();
        return result;
      };
      return true;
    })()`);
    const audioChecks = [];
    async function checkSpectatorAudio(label) {
      const before = await state();
      if (!before.localGameplayState.observerReady) throw new Error("audio sample requires confirmed spectator admission");
      await waitForCombatViewWeaponReady(beta, `${label}: remote shooter ready`);
      const firedBefore = (await beta.metricsState()).uiMetrics.semanticInputCounts.primary;
      const startedAt = await alpha.evaluate("Date.now()");
      await beta.press("Control", 70);
      await waitUntil(`${label}: genuine remote gun dispatch`, async () =>
        (await beta.metricsState()).uiMetrics.semanticInputCounts.primary === firedBefore + 1, 5000);
      let playback;
      await waitUntil(`${label}: nearby game sound reaches running spectator audio`, async () => {
        playback = await alpha.evaluate(`(window.__zubspaceSpectatorAudio || []).filter(r => r.at >= ${startedAt} && r.running && r.volume > 0 && r.duration > 0)`);
        return playback.length > 0;
      }, 5000);
      const after = await state();
      if (after.clientInputSeq !== before.clientInputSeq || !after.localGameplayState.observerReady)
        throw new Error("spectator sound must not open the gameplay input path");
      audioChecks.push({ label, playback, followingId: after.localGameplayState.spectatingPlayerId });
    }
    const enterSpectator = async () => {
      await waitUntil("full live energy before spectator", async () => {
        const m = await state();
        return m.playerAlive === true && m.serverPlayerAlive === true && m.playerMaxEnergy > 0 &&
          m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy &&
          m.localGameplayState?.authorityPhase === "Live";
      }, 15_000);
      await alpha.press("Escape", 70);
      await alpha.press("s", 70);
      await waitUntil("confirmed spectator observer scope", async () => (await state()).localGameplayState.observerReady === true, 10_000);
    };
    const heldListChecks = [];
    async function checkHeldList(label, followingId = null) {
      const before = await state();
      const keys = ["Space", "ArrowRight", "ArrowDown", "Control", "q"];
      try {
        for (const key of keys) await alpha.keyDown(key);
        await waitUntil(`${label}: held list renders`, async () => (await state()).scoreboardVisible, 3000);
        await sleep(450);
        const held = await state();
        if (!held.scoreboardVisible || held.localGameplayState.spectatingPlayerId !== followingId)
          throw new Error(`${label}: list controls changed the spectator target`);
        if (held.clientInputSeq !== before.clientInputSeq || held.positionsSent !== before.positionsSent
          || JSON.stringify(held.semanticInputCounts) !== JSON.stringify(before.semanticInputCounts))
          throw new Error(`${label}: held list opened spectator gameplay or leaked actions`);
        if (followingId === null && Math.hypot(held.playerX-before.playerX, held.playerY-before.playerY) > 1)
          throw new Error(`${label}: held list panned the free camera`);
        screenshots[`player-list-${label}`] = await alpha.screenshot();
        heldListChecks.push({ label, followingId, inputSequenceUnchanged: true, actionCountsUnchanged: true });
      } finally {
        for (const key of keys.toReversed()) await alpha.keyUp(key).catch(() => {});
      }
      await waitUntil(`${label}: release hides list`, async () => !(await state()).scoreboardVisible, 3000);
    }
    await enterSpectator();
    screenshots["free-camera-initial"] = await alpha.screenshot();
    await checkSpectatorAudio("free camera");
    await checkHeldList("free-camera");
    const beforePan = await state();
    for (const [key, axis, sign] of [["ArrowRight", "playerX", 1], ["ArrowLeft", "playerX", -1], ["ArrowDown", "playerY", 1], ["ArrowUp", "playerY", -1]]) {
      const before = await state();
      await alpha.press(key, 450);
      const after = await state();
      if ((after[axis] - before[axis]) * sign < 80) throw new Error(`spectator ${key} did not pan: ${JSON.stringify({before: before[axis], after: after[axis], phase: after.localGameplayState.authorityPhase})}`);
    }
    const afterPan = await state();
    if (afterPan.clientInputSeq !== beforePan.clientInputSeq || afterPan.positionsSent !== beforePan.positionsSent || afterPan.localGameplayState.gameplayScopeReady) throw new Error("spectator camera opened the gameplay/input path");
    // Move away from the clustered spawn so follow must actually relocate the camera.
    await alpha.press("ArrowRight", 900);
    if ((await state()).playerX - afterPan.playerX < 200) throw new Error("follow fixture did not leave the spawn cluster");
    const targets = afterPan.localGameplayState.observerTargetIds;
    if (targets.length !== 3 || !targets.includes(betaId)) throw new Error(`expected human plus two bot targets: ${JSON.stringify(targets)}`);
    const visited = [];
    let movingHumanFollow = null;
    for (let i = 0; i < targets.length; i++) {
      const previous = (await state()).localGameplayState.spectatingPlayerId;
      await alpha.press("Control", 70);
      await waitUntil("spectator cycles to next roster actor", async () => { const id = (await state()).localGameplayState.spectatingPlayerId; return id !== null && id !== previous; }, 5000);
      const id = (await state()).localGameplayState.spectatingPlayerId;
      visited.push(id);
      await waitUntil("spectated actor is published and camera follows", async () => {
        const m = await state(); const target = m.remotePlayers?.find(p => p.id === id);
        return target && Math.hypot(m.playerX - target.positionX, m.playerY - target.positionY) < 80;
      }, 10_000);
      if (id === betaId) {
        await checkSpectatorAudio("following human");
        const before = await state();
        const previousTarget = before.remotePlayers.find(player => player.id === betaId);
        await beta.press("ArrowUp", 650);
        await waitUntil("observer publishes fresh human motion and follows it", async () => {
          const after = await state();
          const target = after.remotePlayers?.find(player => player.id === betaId);
          if (!target) return false;
          const targetTravel = Math.hypot(target.positionX - previousTarget.positionX, target.positionY - previousTarget.positionY);
          const cameraTravel = Math.hypot(after.playerX - before.playerX, after.playerY - before.playerY);
          const cameraError = Math.hypot(after.playerX - target.positionX, after.playerY - target.positionY);
          if (targetTravel <= 24 || cameraTravel <= 16 || cameraError >= 80 || after.latestSnapshotId === before.latestSnapshotId) return false;
          movingHumanFollow = { playerId: betaId, targetTravel, cameraTravel, cameraError,
            beforeSnapshotId: before.latestSnapshotId, afterSnapshotId: after.latestSnapshotId };
          return true;
        }, 10_000);
      }
      screenshots[`following-${id}`] = await alpha.screenshot();
      if (i === 0) await checkHeldList("following", id);
    }
    const botNames = new Map();
    const rosterState = await (alpha.artifactState ? alpha.artifactState() : alpha.pageState());
    for (const row of summarizeClient(rosterState).rosters) {
      if (row.id < 768) continue;
      if (!row.name || row.name.length > 14 || row.name.startsWith("Bot "))
        throw new Error(`invalid curated spectator bot label: ${JSON.stringify(row)}`);
      if (botNames.has(row.id) && botNames.get(row.id) !== row.name)
        throw new Error("spectator roster renamed an existing bot");
      botNames.set(row.id, row.name);
    }
    if (botNames.size !== 2 || new Set(botNames.values()).size !== 2)
      throw new Error(`spectator roster lacks two distinct bot names: ${JSON.stringify([...botNames])}`);
    if (new Set(visited).size !== targets.length || targets.some(id => !visited.includes(id))) throw new Error(`not every human/bot target was followed: ${visited}`);
    const prior = (await state()).localGameplayState.spectatingPlayerId;
    await alpha.press("Tab", 70);
    await waitUntil("Tab selects previous actor", async () => (await state()).localGameplayState.spectatingPlayerId !== prior, 5000);
    await alpha.press("ArrowRight", 450);
    if ((await state()).localGameplayState.spectatingPlayerId !== null) throw new Error("panning did not detach follow");
    screenshots["free-camera-detached"] = await alpha.screenshot();
    await alpha.press("Enter", 70);
    const chatPose = await state();
    await alpha.press("ArrowRight", 250);
    await alpha.press("Space", 70);
    const inChat = await state();
    if (!inChat.chatActive || Math.abs(inChat.playerX - chatPose.playerX) > 1 || inChat.localGameplayState.spectatingPlayerId !== null) throw new Error(`chat did not isolate observer controls: ${JSON.stringify({chat:inChat.chatActive,beforeX:chatPose.playerX,afterX:inChat.playerX,target:inChat.localGameplayState.spectatingPlayerId})}`);
    await alpha.press("Escape", 70);
    await alpha.press("Escape", 70);
    const menuPose = await state();
    await alpha.press("ArrowRight", 250);
    await alpha.press("Space", 70);
    const inMenu = await state();
    if (Math.abs(inMenu.playerX - menuPose.playerX) > 1 || inMenu.localGameplayState.spectatingPlayerId !== null) throw new Error("menu did not isolate observer controls");
    await alpha.press("1", 70);
    await waitUntil("Esc + 1 returns to playable ship", async () => { const m = await state(); return m.playerAlive && m.gameplayStepReady; }, 15_000);
    await rendezvousPlayers(alpha, beta, server);
    await waitForPlayersReady(alpha, beta, server);
    const rejoined = await state();
    await alpha.press("ArrowUp", 450);
    const moved = await state();
    if (Math.hypot(moved.playerX - rejoined.playerX, moved.playerY - rejoined.playerY) < 3) throw new Error("returned ship cannot thrust");
    await sleep(1200);
    await enterSpectator();
    await alpha.press("Escape", 70);
    await alpha.press("8", 70);
    await waitUntil("Esc + 8 returns to playable ship", async () => { const m = await state(); return m.playerAlive && m.gameplayStepReady; }, 15_000);
    screenshots.returned = await alpha.screenshot();
    const result = await evidence.succeed({ ok: true, arena, browser: firefox ? "firefox" : "chrome", spectatorPan: "four arrows", targets, visited,
      fixture: { bots: 2, passiveBotLifeSeqs: [1] }, movingHumanFollow,
      heldListChecks, audioChecks, botNames: Object.fromEntries(botNames),
      botTargets: targets.filter(id => id !== betaId), returnToShip: [1, 8], menuIsolation: true, chatIsolation: true, spectatorGameplayGateClosed: true }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    // Preserve unfiltered starts/gains, including silent or suspended ones.
    // The positive running-volume assertion above remains the verdict.
    failedAudioProbe = await alpha?.evaluate("window.__zubspaceSpectatorAudio || []")
      .catch(probeError => ({ unavailable: probeError.message })) ?? null;
    const artifacts = await evidence.fail(error, capture());
    error.message = `${error.message}\n${browserE2eFailureFooter(artifacts)}`;
    throw error;
  } finally {
    await alpha?.close().catch(() => {});
    await beta?.close().catch(() => {});
    await Promise.all([
      stopProcess(alphaChrome.child),
      stopProcess(betaChrome.child),
      stopProcess(server.child),
    ]).catch(() => {});
    if (alphaChrome.userDataDir) fs.rmSync(alphaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    fs.rmSync(betaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
