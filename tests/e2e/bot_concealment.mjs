#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import { withFixtureSpawnRegion } from "./fixture_spawn_region.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { botDetectorTargetEvidence } from "./bot_detector_evidence.mjs";

const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const exhaustion = process.env.ZUBSPACE_E2E_BOT_DETECTOR_EXHAUSTION === "1";

// Visibility/acquisition coverage on a declared spawn-safety map fixture;
// Normal mode preserves Chaos balance; both declare a safety-patch spawn.
// Exhaustion mode additionally declares its energy
// boundary. Bot weapons stay enabled in both. This is not combat proof.
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/bot_concealment.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  // Ordinary Chaos spawn is not safety. Stationary visibility probes there can
  // die to already airborne shots after hiding, which resets their status.
  // Use real map safety rather than disabling damage, bots, or life assertions.
  const zoneRoot = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-concealment-zone-"));
  const zone = path.join(zoneRoot, "chaos-zone");
  fs.cpSync("assets/zones/chaos-zone", zone, { recursive: true });
  const manifestPath = path.join(zone, "zone.bundle");
  const manifest = fs.readFileSync(manifestPath, "utf8");
  const mapPath = path.join(zone, manifest.match(/^map=(.+)$/m)[1]);
  const original = fs.readFileSync(mapPath);
  const map = Buffer.from(original);
  const offset = map.subarray(0, 2).toString() === "BM" ? map.readUInt32LE(2) : 0;
  const added = [];
  for (let y = 511; y <= 513; y++) for (let x = 511; x <= 513; x++) {
    const record = Buffer.alloc(4);
    record.writeUInt32LE((171 * 2 ** 24 + y * 4096 + x) >>> 0);
    let replaced = false;
    for (let cursor = offset; cursor + 4 <= map.length; cursor += 4) {
      if ((map.readUInt32LE(cursor) & 0xffffff) === y * 4096 + x) {
        record.copy(map, cursor); replaced = true;
      }
    }
    if (!replaced) added.push(record);
  }
  const fixtureMap = Buffer.concat([map, ...added]);
  const hash = bytes => crypto.createHash("sha256").update(bytes).digest("hex");
  fs.writeFileSync(mapPath, fixtureMap);
  fs.writeFileSync(manifestPath, manifest.replace(/^map_sha256=.+$/m, `map_sha256=${hash(fixtureMap)}`));
  const originalSettings = fs.readFileSync(path.join(zone, "arena.conf"), "utf8");
  let section = "";
  const spawnRegion = { x: 512, y: 512, radius: 0 };
  const settings = withFixtureSpawnRegion(exhaustion ? originalSettings.split("\n").map(line => {
    if (line.startsWith("[")) section = line;
    if (/^(InitialRecharge|MaximumRecharge)=/.test(line)) return `${line.split("=")[0]}=0`;
    if (section === "[Shark]" && /^CloakEnergy=/.test(line)) return "CloakEnergy=0\nStealthEnergy=0";
    if (section === "[PrizeWeight]" && /^\w+=\d+$/.test(line)) return `${line.split("=")[0]}=0`;
    return line;
  }).join("\n") : originalSettings, spawnRegion).toString("utf8");
  fs.writeFileSync(path.join(zone, "arena.conf"), settings);
  fs.writeFileSync(manifestPath, fs.readFileSync(manifestPath, "utf8")
    .replace(/^settings_sha256=.+$/m, `settings_sha256=${hash(settings)}`));
  const fixture = { description: "Chaos safety x511..513/y511..513; ordinary moving bots and active weapons" +
    (exhaustion ? "; declared detector boundary: zero initial/maximum recharge, zero Shark cloak/stealth costs and prize weights; unchanged Warbird X-radar cost 25/s" : "; unchanged shipped balance, explicit safety-patch spawn"),
    exhaustion, spawnRegion, originalMapSha256: hash(original), fixtureMapSha256: hash(fixtureMap),
    originalSettingsSha256: hash(originalSettings), settingsSha256: hash(settings) };
  const port = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: 2, zoneRoot, env: {
    ZUBSPACE_SERVER_LOG_FORMAT: "json", ZUBSPACE_SERVER_SIM_OBSERVE: "1",
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT: "true",
    ZUBSPACE_SERVER_BOT_SHIP_TYPES: "1",
  } });
  const browsers = [startChrome(await freePort(), "concealment-actor"), startChrome(await freePort(), "concealment-observer")];
  const base = `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&trace=1&packetlog=1&password=secret`;
  const urls = [`${base}&name=cloakactor`, `${base}&name=cloakobserver`];
  const clients = [], stages = [], screenshots = {}, traces = [];
  let chunk = 0, pendingLine = "";
  function scanTraces() {
    while (chunk < server.logs.length) pendingLine += server.logs[chunk++];
    const lines = pendingLine.split("\n");
    pendingLine = lines.pop();
    for (const line of lines.filter(Boolean)) {
      // Startup failures can be plain-text stderr; retain them in server.log
      // without masking the original test error during artifact capture.
      if (!line.startsWith("{")) continue;
      const log = JSON.parse(line);
      if (log.fields?.message === "server bot movement trace") traces.push(log.fields);
    }
    return traces;
  }
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i === 0 ? "actor" : "observer", client })),
    extraScreenshots: screenshots,
    extraLogs: { fixture: JSON.stringify(fixture, null, 2), "fixture-settings": settings, stages: JSON.stringify(stages, null, 2), "bot-targets": JSON.stringify(scanTraces(), null, 2),
      "actor-browser": browsers[0].logs, "observer-browser": browsers[1].logs },
    disconnect: async () => { await Promise.all(clients.map(client => client.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); },
  });
  const local = async client => (await client.metricsState()).uiMetrics;
  try {
    await waitForServerListening(port, server.child, server.logs);
    const serverStatus = await (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
    assert.equal(serverStatus.maxed_spawn_loadout, true, "both profiles retain maximum spawn capabilities");
    await Promise.all(browsers.map(waitForChrome));
    // Join both humans together so the exhaustion measurement does not begin
    // with a long surplus-bot combat interval. Assertions use actual assigned
    // player IDs and do not depend on which browser wins the join race.
    clients.push(...await Promise.all(browsers.map((browser, i) => openChromeClient(browser, urls[i]))));
    await Promise.all(clients.map(client => client.enterShip()));
    const [actor, observer] = clients;
    await waitForPlayersReady(actor, observer, server);
    for (let i = 0; i < clients.length; i++) {
      const client = clients[i], witnessClient = clients[1 - i];
      await waitUntil("full energy for Shark selection", async () => {
        const m = await local(client);
        return m.playerAlive && m.gameplayStepReady && m.localGameplayState.authorityPhase === "Live"
          && m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy;
      }, 15_000);
      await client.press("Escape"); await client.press("8");
      await waitUntil("Shark authoritative ship and loadout", async () => {
        const a = await local(client);
        const frame = await witnessClient.evaluate("window.__zubspaceRemoteSpriteTrace?.at(-1)");
        return frame?.entries?.some(entry => entry.id === a.selfPlayerId && entry.ship === 7)
          && a.localGameplayState.authorityPhase === "Live";
      }, 15_000);
      // A real facing input makes this long-running witness an active life;
      // no spawn-idle exemption or departure from the safety patch is needed.
      await client.press("ArrowRight", 70);
    }
    const actorInitial = await local(actor), observerInitial = await local(observer);
    const actorId = actorInitial.selfPlayerId, observerId = observerInitial.selfPlayerId;
    assert.notEqual(actorId, observerId, "the two humans must have distinct authority identities");
    assert.ok(actorInitial.localGameplayState.abilities.stealthCapable && actorInitial.localGameplayState.abilities.cloakCapable);
    assert.ok(observerInitial.localGameplayState.abilities.xradarCapable);
    const actorLife = actorInitial.playerLifeSeq, observerLife = observerInitial.playerLifeSeq;
    let observerConcealed = false;
    let initialDetectorEvidence;
    async function stage(label, bits, world, radar, xradar = false) {
      let actorView, witness;
      await waitUntil(label, async () => {
        const m = await local(observer), a = await local(actor);
        assert.equal(a.playerLifeSeq, actorLife);
        assert.equal(m.playerLifeSeq, observerLife);
        assert.ok(a.inSafeZone && m.inSafeZone, "stationary actors must remain on fixture safety");
        actorView = m.remoteActorPresentation.find(entry => entry.id === actorId);
        const frame = await observer.evaluate("window.__zubspaceRemoteSpriteTrace?.at(-1)");
        const sprite = frame?.entries?.find(entry => entry.id === actorId);
        witness = { bits: actorView?.togglables & 3, drawn: sprite?.drawn,
          radar: m.radarPlayerIds.includes(actorId), xradar: m.localGameplayState.abilities.xradar,
          tick: m.latestSnapshotServerTick };
        return witness.bits === bits && (sprite?.togglables & 3) === bits
          && witness.drawn === world && witness.radar === radar && witness.xradar === xradar;
      }, 10_000);
      const barrier = witness.tick + 20;
      let samples, detectorEvidence;
      await waitUntil(`${label}: sustained authoritative bot target witnesses`, () => {
        samples = scanTraces().filter(trace => trace.server_tick >= barrier);
        detectorEvidence = botDetectorTargetEvidence(samples, { sinceTick: barrier,
          statusByHuman: new Map([[actorId, bits], [observerId, observerConcealed ? 3 : 0]]),
          requireSelectedActive: !exhaustion || !initialDetectorEvidence });
        return detectorEvidence;
      }, 10_000);
      // Match the actual detector used by AI to its published snapshot bit.
      await waitUntil(`${label}: bot detector bits match authority`, async () => {
        const m = await local(observer);
        return detectorEvidence.bots.every(bot => {
          const remote = m.remoteActorPresentation.find(entry => entry.id === bot.id);
          return remote && Boolean(remote.togglables & 4) === bot.activeAtEnd;
        });
      }, 5000);
      initialDetectorEvidence ||= detectorEvidence;
      stages.push({ label, witness, detectorEvidence, botWitnessCount: samples.length, traces: samples });
      screenshots[label] = await observer.screenshot();
      console.log(JSON.stringify({ stage: label, bits, world, radar, xradar, botWitnesses: samples.length }));
    }
    await stage("visible", 0, true, true);
    await actor.press("6");
    await stage("stealth-only", 1, true, false);
    await actor.press("6");
    await actor.press("7");
    await stage("cloak-only", 2, false, true);
    await actor.press("6");
    await stage("fully-hidden", 3, false, false);
    await observer.press("8");
    await stage("observer-xradar-reveal", 3, true, true, true);
    await observer.press("8");
    await stage("observer-xradar-off", 3, false, false);
    await observer.press("6"); await observer.press("7"); observerConcealed = true;
    await stage("both-fully-hidden", 3, false, false);
    if (exhaustion) {
      const selected = initialDetectorEvidence.bots.find(bot => bot.selected);
      assert.ok(selected.activeAtEnd, "the selected detector must have been seen active before exhaustion");
      let off, onSamples;
      await waitUntil("selected bot exhausts its authored detector energy", () => {
        const samples = scanTraces().filter(t => t.bot_id === selected.id &&
          t.server_tick >= selected.firstTick + 600);
        assert.ok(samples.every(t => t.life_seq === selected.life), "bot must not die/reset during drain measurement");
        assert.ok(samples.every(t => t.bot_recharge_per_second === 0));
        onSamples = samples.filter(t => t.bot_xradar_active);
        off = samples.find(t => !t.bot_xradar_active);
        return off && onSamples.length >= 5 && onSamples.at(-1).server_tick - onSamples[0].server_tick >= 500;
      }, 80_000);
      const start = onSamples[0], end = onSamples.at(-1);
      const cost = start.bot_xradar_energy_per_second;
      assert.equal(cost, 25, "the declared fixture preserves authored Warbird X-radar cost");
      assert.ok(onSamples.every(t => t.bot_xradar_energy_per_second === cost));
      const expectedDrop = (end.server_tick - start.server_tick) / 100 * cost;
      assert.ok(Math.abs(start.bot_energy - end.bot_energy - expectedDrop) <= 2,
        "selected bot energy must match authored 25/s drain without regeneration or weapon debits");
      assert.ok(off.bot_energy <= 1 && off.bot_xradar_selected && off.bot_xradar_capable);
      stages.push({ label: "bot-detector-exhaustion", start, end, off, expectedDrop });
      await stage("both-hidden-after-detector-exhaustion", 3, false, false);
      const tail = scanTraces().filter(t => t.bot_id === selected.id && t.server_tick >= off.server_tick);
      assert.ok(tail.every(t => !t.bot_xradar_active), "exhausted detector must not flicker back on");
    }
    await observer.press("6"); await observer.press("7"); observerConcealed = false;
    await stage("observer-world-and-radar-restored", 3, false, false);
    await actor.press("6");
    await stage("radar-reacquisition", 2, false, true);
    await actor.press("7");
    await stage("world-reacquisition", 0, true, true);
    for (const client of clients) {
      const state = await client.pageState();
      assert.equal(state.uiMetrics.localGameplayState.rtcRejoinCount, 0);
      assert.ok(!state.networkLog.some(entry => entry.level === "error"));
      assert.ok(!client.events.some(event => ["error", "exception"].includes(event.type)));
    }
    const result = await evidence.succeed({ ok: true, actorId, observerId, fixture,
      coverage: "genuine ability keys; human world/radar visibility; balanced bot active detector bits, actual acquisition and pursuit for both hidden humans; independent observer X-radar; hide/reacquire without reconnect" + (exhaustion ? "; authored bot drain and detector exhaustion under declared boundary settings" : "; unchanged shipped Chaos settings"), stages }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    error.message += `\n${browserE2eFailureFooter(artifacts)}`;
    throw error;
  } finally {
    await Promise.all(clients.map(client => client.close().catch(() => {})));
    await Promise.all([...browsers.map(browser => stopProcess(browser.child)), stopProcess(server.child)]).catch(() => {});
    for (const browser of browsers) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
    fs.rmSync(zoneRoot, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
