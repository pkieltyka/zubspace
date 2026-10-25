#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";

import fs from "node:fs";
import path from "node:path";

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
  browserE2eFailureFooter,
  parseHiddenPrizeSpawns,
  parsePrizeClaims,
  openChromeClient,
  startChrome,
  waitForChrome,
  waitForLivePlayerScopes,
  waitUntil,
} from "./combat_replay.mjs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";
import { loadRendezvousMap } from "./player_rendezvous.mjs";
import { planNaturalPrizeNavigation } from "./natural_prize_navigation.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const e2eScript = "tests/e2e/timed_prize_visuals.mjs";

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  // With current Chaos weights and two pilots, the first batch includes
  // Super at (8120,8168) and Shields at (8248,8200), both on clear tiles.
  process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED = "4750";
  // This visual-effect lane owns short natural-prize approaches. Declare
  // only its private initial region; shipping random spawns and TTL stay put.
  const spawnFixture = createSpawnRegionFixture({ zoneId: arena,
    spawnRegion: { x: 512, y: 512, radius: 2 } });
  const loadedMap = loadRendezvousMap(arena, [spawnFixture.root]);
  const evidence = createFocusedBrowserEvidence({ e2eScript });

  const serverPort = await freePort();
  const alphaPort = await freePort();
  const betaPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort, { zoneRoot: spawnFixture.root,
    env: { ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" } });
  const alphaChrome = startChrome(alphaPort, "timed-prize-alpha");
  const betaChrome = startChrome(betaPort, "timed-prize-beta");
  const screenshots = {};
  let alpha = null;
  let beta = null;
  let alphaUrl = null;
  let betaUrl = null;
  const capture = () => ({
    server, serverPort, staticServer, urls: [alphaUrl, betaUrl],
    clients: [{ label: "alpha", client: alpha }, { label: "beta", client: beta }],
    extraScreenshots: screenshots,
    extraLogs: { "alpha-chrome": alphaChrome.logs, "beta-chrome": betaChrome.logs,
      "timed-visual-spawn-fixture": JSON.stringify(spawnFixture),
      "timed-visual-scenario-settings": fs.readFileSync(path.join(spawnFixture.root, arena, "arena.conf"), "utf8"),
      "timed-visual-zone-bundle": fs.readFileSync(path.join(spawnFixture.root, arena, "zone.bundle"), "utf8") },
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
    const policy = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
    if (policy.free_for_all !== true) throw new Error("visual gate requires default FFA policy");
    await Promise.all([waitForChrome(alphaChrome), waitForChrome(betaChrome)]);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase =
      `http://127.0.0.1:${staticServer.port}/index.html` +
      `?rtc=${encodeURIComponent(rtcUrl)}` +
      `&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1`;
    alphaUrl = `${pageBase}&name=menualpha`;
    betaUrl = `${pageBase}&name=menubeta&debug=31337`;
    [alpha, beta] = await Promise.all([
      openChromeClient(alphaChrome, alphaUrl),
      openChromeClient(betaChrome, betaUrl),
    ]);
    await Promise.all([alpha.enterShip(), beta.enterShip()]);
    // Local timed-effect readiness is independent of mutual AOI visibility.
    await waitForLivePlayerScopes(alpha, beta);
    await beta.press("ArrowRight", 70);

    const playerId = (await alpha.metricsState()).uiMetrics.selfPlayerId;
    screenshots["before-pickups"] = await alpha.screenshot();
    const activations = [];
    for (const [prizeId, timerField, name] of [[18, "shieldTimer", "shield"], [17, "superTimer", "super"]]) {
      const deadline = Date.now() + 60000;
      let claim = null;
      const routes = [];
      while (Date.now() < deadline && !claim) {
        const claimed = parsePrizeClaims(server);
        const state = await alpha.metricsState();
        const metrics = state.uiMetrics;
        if (metrics.localGameplayState?.brickTilesOverflow) throw new Error("timed visual preparation has incomplete brick geometry");
        const targets = parseHiddenPrizeSpawns(server).filter(prize => prize.prizeId === prizeId &&
          !claimed.some(done => done.collectibleId === prize.collectibleId));
        let target = null, navigation = null;
        for (const candidate of targets) {
          let planned;
          try {
            planned = planNaturalPrizeNavigation(loadedMap.geometry,
              { x: metrics.playerX, y: metrics.playerY }, candidate, metrics.latestSnapshotServerTick,
              metrics.localGameplayState?.brickTiles || []);
          } catch (error) {
            if (/no hull-clear authored approach route/.test(error.message)) continue;
            throw error;
          }
          if (planned && planned.requiredTtlTicks <= Math.min(2000, Math.floor((deadline-Date.now()) / 10)) &&
              (!navigation || planned.plan.routes[0].distancePx < navigation.plan.routes[0].distancePx)) {
            target = candidate; navigation = planned;
          }
        }
        if (!target) { await sleep(100); continue; }
        routes.push(await navigateAuthoredTarget(alpha, target, server, {
          timeoutMs: Math.min(20000, deadline-Date.now()),
          arrivalRadiusPx: 16, maximumDesiredSpeed: 80, desiredSpeedScale: 0.5,
          complete: async () => parsePrizeClaims(server).find(done => done.playerId === playerId && done.collectibleId === target.collectibleId && done.prizeId === prizeId) || null,
        }));
        routes.at(-1).prizeNavigation = navigation;
        claim = parsePrizeClaims(server).find(done => done.playerId === playerId &&
          done.collectibleId === target.collectibleId && done.prizeId === prizeId);
      }
      if (!claim) throw new Error(`natural ${name} pickup not collected: ${JSON.stringify({routes,spawns:parseHiddenPrizeSpawns(server)})}`);
      let activation;
      await waitUntil(`${name} receipt is active`, async () => {
        activation = (await alpha.metricsState()).uiMetrics.localGameplayState;
        return activation[timerField] > 0;
      }, 5000);
      screenshots[`${name}-active`] = await alpha.screenshot();
      activations.push({name, claim, timer:activation[timerField], revision:activation.prizeAuthorityRevision});
    }
    const result = await evidence.succeed({
      ok: true, transport: "rtc", arena,
      activations, spawnFixture, ordinaryDebugMode: false,
    }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    const originalStack = error.stack || error.message;
    const footer = browserE2eFailureFooter(artifacts);
    error.message = `${error.message}\n${footer}`;
    error.stack = `${originalStack}\n${footer}`;
    throw error;
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
    fs.rmSync(spawnFixture.root, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
