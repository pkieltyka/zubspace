#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady, rendezvousPlayers, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";

const names = ["Warbird", "Javelin", "Spider", "Leviathan", "Terrier", "Weasel", "Lancaster", "Shark"];
const statusNames = ["stealth", "cloak", "xradar", "antiwarp"];
const mask = local => statusNames.reduce((bits, name, i) => bits | (local.abilities[name] ? 1 << i : 0), 0);
const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");

// Shipped Chaos map/balance with an explicit private audit station outside
// the natural prize field. No prize grant, damage hook or shortened timer.
// Retain product maximum loadouts and authored disabled-cloak behavior.
async function main() {
  // Retain the original sparse green seed. The unchanged zero-pickup
  // assertions also reject any incidental acquisition during real setup travel.
  process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED = "20";
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/ship_abilities.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  // Every ship-change life starts at the same clear station, avoiding a new
  // random route through ambient greens before its starting-loadout audit.
  // Prize spawning and all ship/ability/balance settings remain unchanged.
  const spawnFixture = createSpawnRegionFixture({ zoneId: "chaos-zone",
    spawnRegion: { x: 544, y: 512, radius: 0 } });
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { botCount: 0, zoneRoot: spawnFixture.root,
    env: { ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" } });
  const browsers = [startChrome(await freePort(), "ship-abilities-actor"), startChrome(await freePort(), "ship-abilities-observer")];
  const base = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&trace=1&packetlog=1&password=secret`;
  const urls = [`${base}&name=abilitypilot`, `${base}&name=abilitywitness`];
  const clients = [], ships = [], screenshots = {};
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i ? "observer" : "actor", client })),
    extraScreenshots: screenshots, extraLogs: { ships: JSON.stringify(ships, null, 2),
      "spawn-fixture": JSON.stringify(spawnFixture, null, 2),
      "scenario-settings": fs.readFileSync(path.join(spawnFixture.root, "chaos-zone/arena.conf"), "utf8"),
      "scenario-zone-bundle": fs.readFileSync(path.join(spawnFixture.root, "chaos-zone/zone.bundle"), "utf8"),
      "actor-browser": browsers[0].logs, "observer-browser": browsers[1].logs },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); },
  });
  const metrics = async client => (await client.metricsState()).uiMetrics;
  try {
    await waitForServerListening(port, server.child, server.logs);
    const serverStatus = await (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
    assert.equal(serverStatus.maxed_spawn_loadout, true, "production ability audit requires the default maxed starting loadout");
    await Promise.all(browsers.map(waitForChrome));
    for (let i = 0; i < browsers.length; i++) {
      clients.push(await openChromeClient(browsers[i], urls[i])); await clients[i].enterShip();
    }
    const [actor, observer] = clients;
    await rendezvousPlayers(actor, observer, server, { stageRadiusPx: 20 });
    await waitForPlayersReady(actor, observer, server);
    const actorId = (await metrics(actor)).selfPlayerId;
    const observerLife = (await metrics(observer)).playerLifeSeq;
    let life = (await metrics(actor)).playerLifeSeq;
    async function witness() {
      const [a, b] = await Promise.all(clients.map(metrics));
      assert.equal(b.playerLifeSeq, observerLife, "idle witness must keep its original life");
      const remote = b.remoteActorPresentation.find(p => p.id === actorId);
      return { a, b, local: a.localGameplayState, remote, active: mask(a.localGameplayState), remoteActive: remote?.togglables & 15 };
    }
    async function fullEnergy() {
      await waitUntil("full client/server energy before ship selection", async () => {
        const m = await metrics(actor); return m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy;
      }, 10_000);
    }
    for (let ship = 0; ship < 8; ship++) {
      await fullEnergy();
      if (ship !== 0) {
        await actor.press("Escape"); await actor.press(String(ship+1));
        await waitUntil("replacement ship admits its exact successor life", async () => {
          const m = await metrics(actor), local = m.localGameplayState;
          return m.playerLifeSeq > life && local.gameplayStepReady
            && local.prizeAuthorityLifeSeq === m.playerLifeSeq;
        }, 15_000);
        await rendezvousPlayers(actor, observer, server, { stageRadiusPx: 20 });
        await waitForPlayersReady(actor, observer, server);
      }
      let ready;
      await waitUntil(`${names[ship]} authoritative life and initial inventory`, async () => {
        ready = await witness();
        const frame = await observer.evaluate("window.__zubspaceRemoteSpriteTrace?.at(-1)");
        return frame?.entries?.some(p => p.id === actorId && p.ship === ship)
          && ready.local.authorityPhase === "Live" && !ready.local.networkPrizeBaselinePending
          && ready.local.prizeAuthorityLifeSeq === ready.a.playerLifeSeq
          && (ship === 0 ? ready.a.playerLifeSeq === life : ready.a.playerLifeSeq > life);
      }, 15_000);
      life = ready.a.playerLifeSeq;
      assert.equal(ready.local.prizePickupNotificationCount, 0, "audit starting loadouts without ambient pickups");
      const cloak = ship === 2 || ship === 7;
      assert.deepEqual(ready.local.abilities, {
        multifireCapable: true, multifire: false, stealthCapable: true, stealth: false,
        cloakCapable: cloak, cloak: false, xradarCapable: true, xradar: false,
        antiwarpCapable: true, antiwarp: false,
      });
      assert.equal(ready.active, 0); assert.equal(ready.remoteActive, 0);
      assert.equal(ready.a.playerMaxEnergy, 1700);
      assert.equal(ready.local.rechargePerSecond, 115);
      assert.equal(ready.local.baseTopSpeed, ship === 1 ? 375 : 325);
      assert.equal(ready.local.baseThrustAcceleration, ship === 0 ? 190 : ship === 1 ? 170 : 180);
      assert.equal(ready.local.gunLevel, 3);
      assert.equal(ready.local.bombLevel, ship === 3 ? 3 : 2);
      for (const item of ["repels", "bursts", "decoys", "thors", "rockets", "portals"]) {
        assert.equal(ready.local[item], 3, `${names[ship]} maximum ${item}`);
      }
      assert.equal(ready.local.bricks, ship === 6 ? 100 : 3);
      const transitions = [];
      async function status(key, expected, label) {
        const before = await witness(); await actor.press(key);
        let after;
        await waitUntil(`${names[ship]} ${label}`, async () => {
          after = await witness(); assert.equal(after.a.playerLifeSeq, life);
          return after.active === expected && after.remoteActive === expected;
        }, 5000);
        transitions.push({ key, label, expected, tick: after.a.latestSnapshotServerTick,
          serverEnergy: after.a.serverPlayerEnergy, predictedEnergy: after.a.playerPredictedEnergy });
        if (key === "7" || key === "9") {
          const counter = key === "7" ? "cloak" : "antiwarp";
          assert.equal(after.a.semanticInputCounts[counter], before.a.semanticInputCounts[counter]+1);
        }
      }
      await status("6", 1, "stealth on"); await status("6", 0, "stealth off");
      await status("8", 4, "X-radar on"); await status("8", 0, "X-radar off");
      await status("7", cloak ? 2 : 0, cloak ? "cloak on" : "cloak unavailable");
      if (cloak) await status("7", 0, "cloak off");
      await status("9", 8, "Antiwarp on"); await status("9", 0, "Antiwarp off");
      for (const [key, enabled] of [["-", true], ["Delete", false]]) {
        await actor.press(key);
        await waitUntil(`${names[ship]} multifire ${enabled ? "on" : "off"}`, async () => {
          const current = await witness();
          assert.equal(current.a.playerLifeSeq, life);
          assert.equal(current.local.abilities.multifireCapable, true);
          return current.local.abilities.multifire === enabled;
        }, 5000);
      }
      await status("6", 1, "combined stealth");
      await status("8", 5, "combined X-radar");
      await status("9", 13, "combined Antiwarp");
      if (cloak) await status("7", 15, "combined cloak");
      const start = await witness(), samples = [];
      await waitUntil(`${names[ship]} authored recharge sustains owned status costs`, async () => {
        const current = await witness(); assert.equal(current.a.playerLifeSeq, life);
        assert.equal(current.active, cloak ? 15 : 13); assert.equal(current.remoteActive, current.active);
        assert.ok(current.a.serverPlayerEnergy >= 1690 && current.a.playerPredictedEnergy >= 1690);
        samples.push({ tick: current.a.latestSnapshotServerTick, serverEnergy: current.a.serverPlayerEnergy,
          predictedEnergy: current.a.playerPredictedEnergy, active: current.active, remoteActive: current.remoteActive });
        return current.a.latestSnapshotServerTick-start.a.latestSnapshotServerTick >= 150
          && samples.some(s => s.serverEnergy < 1700 && s.predictedEnergy < 1700);
      }, 5000);
      await status("6", cloak ? 14 : 12, "restore stealth off");
      await status("8", cloak ? 10 : 8, "restore X-radar off");
      await status("9", cloak ? 2 : 0, "restore Antiwarp off");
      if (cloak) await status("7", 0, "restore cloak off");
      await fullEnergy();
      assert.equal((await metrics(actor)).localGameplayState.prizePickupNotificationCount, 0);
      screenshots[names[ship].toLowerCase()] = await actor.screenshot();
      ships.push({ ship, name: names[ship], life, abilities: ready.local.abilities,
        startingInventory: ready.local, transitions, rechargeSamples: samples });
      console.log(JSON.stringify({ ship: names[ship], life, cloak, abilityChecks: "passed", rechargeSamples: samples.length }));
    }
    for (const client of clients) {
      const s = await client.pageState(); assert.equal(s.uiMetrics.localGameplayState.rtcRejoinCount, 0);
      assert.ok(!s.networkLog.some(e => e.level === "error"));
      assert.ok(!client.events.some(e => ["error", "exception"].includes(e.type)));
    }
    console.log(JSON.stringify(await evidence.succeed({ ok: true, arena: "chaos-zone", actorId, hiddenPrizeSeed: 20,
      coverage: "all eight actual Chaos maxed-spawn baselines; owned/inactive capabilities; positive and unavailable genuine keys; local/opponent status agreement; real recharge sustains authored costs; explicit ship-selection life transitions; original RTC", ships }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture()); error.message += `\n${browserE2eFailureFooter(artifacts)}`; throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    for (const b of browsers) fs.rmSync(b.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
    fs.rmSync(spawnFixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
