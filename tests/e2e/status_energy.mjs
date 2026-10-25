#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import { withFixtureSpawnRegion } from "./fixture_spawn_region.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { repoRoot, freePort, sleep, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

const abilities = ["stealth", "cloak", "xradar", "antiwarp"];
const mask = state => abilities.reduce((bits, name, i) => bits | (state.abilities[name] ? 1 << i : 0), 0);
const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");

async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/status_energy.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  // Keep actual Chaos costs and ship/map/weapon rules. Isolate exhaustion with
  // no starting prize rolls, no recharge, owned-but-inactive optional statuses,
  // and no ambient greens that could recharge or remove a capability mid-run.
  // This is a declared boundary fixture, not a change to shipped balance.
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-status-energy-"));
  const zone = path.join(root, "chaos-zone");
  fs.cpSync("assets/zones/chaos-zone", zone, { recursive: true });
  const original = fs.readFileSync(path.join(zone, "arena.conf"), "utf8");
  let section = "";
  const spawnRegion = { x: 512, y: 512, radius: 0 };
  const settings = withFixtureSpawnRegion(original.split("\n").map(line => {
    if (line.startsWith("[")) section = line;
    if (/^InitialBounty=/.test(line)) return "InitialBounty=0";
    if (/^InitialRecharge=/.test(line)) return "InitialRecharge=0";
    if (/^(Stealth|XRadar|AntiWarp)Status=/.test(line)) return `${line.split("=")[0]}=2`;
    if (section === "[PrizeWeight]" && /^\w+=\d+$/.test(line)) return `${line.split("=")[0]}=0`;
    return line;
  }).join("\n"), spawnRegion).toString("utf8");
  const hash = data => crypto.createHash("sha256").update(data).digest("hex");
  fs.writeFileSync(path.join(zone, "arena.conf"), settings);
  const manifestPath = path.join(zone, "zone.bundle");
  fs.writeFileSync(manifestPath, fs.readFileSync(manifestPath, "utf8").replace(/^settings_sha256=.+$/m, `settings_sha256=${hash(settings)}`));
  const fixture = { description: "Explicit central radius0 fixture spawn; Chaos costs/map/ship/weapon rules; InitialBounty=0, InitialRecharge=0, Stealth/XRadar/AntiWarpStatus=2, all prize weights zero; Shark's existing CloakStatus=2",
    maxedSpawnLoadout: false, spawnRegion, originalSettingsSha256: hash(original), settingsSha256: hash(settings),
    rates: { stealth: 33.3, cloak: 10, xradar: 25, antiwarp: 10 }, recharge: 0 };
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { botCount: 0, zoneRoot: root, env: { ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT: "false", ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" } });
  const browsers = [startChrome(await freePort(), "status-energy-actor"), startChrome(await freePort(), "status-energy-observer")];
  const base = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&trace=1&packetlog=1&password=secret`;
  const urls = [`${base}&name=energyactor`, `${base}&name=energyobserver`];
  const clients = [], stages = [], samples = [], screenshots = {};
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i ? "observer" : "actor", client })),
    extraScreenshots: screenshots,
    extraLogs: { stages: JSON.stringify(stages, null, 2), samples: JSON.stringify(samples, null, 2),
      fixture: JSON.stringify(fixture, null, 2), "fixture-settings": settings,
      "actor-browser": browsers[0].logs, "observer-browser": browsers[1].logs },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); },
  });
  const metrics = async client => (await client.metricsState()).uiMetrics;
  try {
    await waitForServerListening(port, server.child, server.logs);
    const serverStatus = await (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
    assert.equal(serverStatus.maxed_spawn_loadout, false, "declared acquisition/exhaustion fixture must use reference starting loadouts");
    await Promise.all(browsers.map(waitForChrome));
    for (let i = 0; i < browsers.length; i++) {
      clients.push(await openChromeClient(browsers[i], urls[i])); await clients[i].enterShip();
    }
    const [actor, observer] = clients;
    await waitForPlayersReady(actor, observer, server);
    await actor.press("Escape"); await actor.press("8");
    let initial;
    await waitUntil("Shark's owned status loadout is live and inactive", async () => {
      initial = await metrics(actor);
      const frame = await observer.evaluate("window.__zubspaceRemoteSpriteTrace?.at(-1)");
      return frame?.entries?.some(p => p.id === initial.selfPlayerId && p.ship === 7)
        && initial.localGameplayState.authorityPhase === "Live"
        && abilities.every(name => initial.localGameplayState.abilities[`${name}Capable`])
        && mask(initial.localGameplayState) === 0;
    }, 15_000);
    assert.equal(initial.localGameplayState.rechargePerSecond, 0);
    screenshots["ship-normal"] = await actor.screenshot();
    const actorId = initial.selfPlayerId, life = initial.playerLifeSeq;
    const observerLife = (await metrics(observer)).playerLifeSeq;
    async function witness() {
      const [a, b] = await Promise.all(clients.map(metrics));
      assert.equal(a.playerLifeSeq, life); assert.equal(b.playerLifeSeq, observerLife);
      const remote = b.remoteActorPresentation.find(p => p.id === actorId);
      assert.ok(remote, "opponent must retain this actor's authority presentation");
      return { tick: a.latestSnapshotServerTick, serverEnergy: a.serverPlayerEnergy,
        predictedEnergy: a.playerPredictedEnergy, active: mask(a.localGameplayState),
        remoteActive: remote.togglables & 15, capabilities: a.localGameplayState.abilities };
    }
    async function status(key, expected, label) {
      const before = await witness();
      await actor.press(key);
      let after;
      await waitUntil(label, async () => {
        after = await witness(); return after.active === expected && after.remoteActive === expected;
      }, 5000);
      stages.push({ label, key, before, after });
      if (["cloak-on", "cloak-off", "stealth-on"].includes(label)) {
        screenshots[`ship-${label}`] = await actor.screenshot();
      }
    }
    // Independent activation/deactivation before the combined-cost boundary.
    for (const [i, key] of ["6", "7", "8", "9"].entries()) {
      await status(key, 1 << i, `${abilities[i]}-on`);
      await status(key, 0, `${abilities[i]}-off`);
    }
    // Reveal cloaked actors only to this observer, not by altering the actor.
    await observer.press("8");
    await waitUntil("observer X-radar active", async () => (await metrics(observer)).localGameplayState.abilities.xradar, 3000);
    let active = 0;
    for (const [i, key] of ["6", "7", "8", "9"].entries()) {
      active |= 1 << i;
      await status(key, active, `combined-add-${abilities[i]}`);
    }
    const start = await witness(), rate = Object.values(fixture.rates).reduce((a, b) => a + b, 0);
    screenshots["all-statuses-active"] = await actor.screenshot();
    let measuredSlope = false, allOff;
    await waitUntil("energy exhaustion automatically disables all four statuses", async () => {
      const current = await witness(); samples.push(current);
      assert.ok(abilities.every(name => current.capabilities[`${name}Capable`]), "exhaustion must not remove capabilities");
      if (current.active === 15 && current.remoteActive === 15 && current.tick - start.tick >= 500) {
        const expectedDrop = (current.tick-start.tick)/100*rate;
        assert.ok(Math.abs((start.serverEnergy-current.serverEnergy)-expectedDrop) <= 25,
          `server drain must match authored 78.3 energy/sec: ${JSON.stringify({start,current,expectedDrop})}`);
        measuredSlope = true;
      }
      if (!screenshots["yellow-energy"] && current.predictedEnergy < 850 && current.predictedEnergy > 650) {
        screenshots["yellow-energy"] = await actor.screenshot();
      }
      if (!screenshots["red-energy"] && current.predictedEnergy < 400 && current.predictedEnergy > 200) {
        screenshots["red-energy"] = await actor.screenshot();
        screenshots["opponent-red-energy"] = await observer.screenshot();
      }
      allOff = current;
      return current.active === 0 && current.remoteActive === 0 && current.serverEnergy < 1 && current.predictedEnergy < 1;
    }, 35_000);
    assert.ok(measuredSlope, "retain a five-second measured authored-cost window");
    assert.ok(samples.some(s => s.active === 15) && samples.some(s => s.active !== 15));
    const until = Date.now()+2000;
    while (Date.now() < until) {
      const current = await witness(); samples.push(current);
      assert.equal(current.active, 0); assert.equal(current.remoteActive, 0);
      assert.ok(abilities.every(name => current.capabilities[`${name}Capable`]));
      await sleep(100);
    }
    stages.push({ label: "authored-cost-depletion", rate, start, allOff });
    screenshots["all-statuses-exhausted"] = await actor.screenshot();
    for (const client of clients) {
      const s = await client.pageState();
      assert.equal(s.uiMetrics.localGameplayState.rtcRejoinCount, 0);
      assert.ok(!s.networkLog.some(e => e.level === "error"));
      assert.ok(!client.events.some(e => ["error", "exception"].includes(e.type)));
    }
    console.log(JSON.stringify(await evidence.succeed({ ok: true, fixture, actorId,
      coverage: "genuine independent/combined ability keys; authored Chaos status costs under declared zero-recharge boundary; server/prediction/observer exhaustion; owned capability retained; original life and RTC", stages, measuredSlope }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture()); error.message += `\n${browserE2eFailureFooter(artifacts)}`; throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    for (const b of browsers) fs.rmSync(b.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close(); fs.rmSync(root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
