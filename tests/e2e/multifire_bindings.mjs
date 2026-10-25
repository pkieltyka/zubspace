#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import { withFixtureSpawnRegion } from "./fixture_spawn_region.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { freePort, sleep, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { parseServerProjectileEvents } from "../../tools/server_events.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady,
  parseHiddenPrizeSpawns, parsePrizeClaims, brakeClientVelocity,
  orientClientForProtocolDirection, browserE2eFailureFooter } from "./combat_replay.mjs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

async function main() {
  process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED = "220";
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/multifire_bindings.mjs" });
  // Chaos deliberately has zero MultiFire weight. This declared fixture changes
  // prize selection, not production balance: no starting bounty, only MultiFire
  // greens, no negative greens. Map, ships, weapons and authority stay intact.
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-multifire-zone-"));
  const zone = path.join(root, "chaos-zone");
  fs.cpSync("assets/zones/chaos-zone", zone, { recursive: true });
  const original = fs.readFileSync(path.join(zone, "arena.conf"), "utf8");
  let section = "";
  const spawnRegion = { x: 512, y: 512, radius: 0 };
  const settings = withFixtureSpawnRegion(original.split("\n").map(line => {
    if (line.startsWith("[")) section = line;
    if (/^InitialBounty=/.test(line)) return "InitialBounty=0";
    if (/^PrizeNegativeFactor=/.test(line)) return "PrizeNegativeFactor=0";
    if (section === "[PrizeWeight]" && /^\w+=\d+$/.test(line)) return `${line.split("=")[0]}=${line.startsWith("MultiFire=") ? 255 : 0}`;
    return line;
  }).join("\n"), spawnRegion).toString("utf8");
  const hash = value => crypto.createHash("sha256").update(value).digest("hex");
  fs.writeFileSync(path.join(zone, "arena.conf"), settings);
  const manifestPath = path.join(zone, "zone.bundle");
  fs.writeFileSync(manifestPath, fs.readFileSync(manifestPath, "utf8").replace(/^settings_sha256=.+$/m, `settings_sha256=${hash(settings)}`));
  const fixture = { description: "Explicit central radius0 fixture spawn; Original Chaos map/ship/weapon settings; InitialBounty=0; only MultiFire prize weight=255; PrizeNegativeFactor=0",
    maxedSpawnLoadout: false, spawnRegion, originalSettingsSha256: hash(original), settingsSha256: hash(settings) };
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { botCount: 0, zoneRoot: root, env: { ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT: "false", ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" } });
  const browsers = [startChrome(await freePort(), "multifire-actor"), startChrome(await freePort(), "multifire-observer")];
  const base = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&trace=1&packetlog=1&password=secret`;
  const urls = [`${base}&name=multiactor`, `${base}&name=multiobserver`];
  const clients = [], stages = [], screenshots = {};
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i ? "observer" : "actor", client })),
    extraScreenshots: screenshots,
    extraLogs: { stages: JSON.stringify(stages, null, 2), fixture: JSON.stringify(fixture, null, 2), "fixture-settings": settings,
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
    const initial = await metrics(actor), observerInitial = await metrics(observer);
    const actorId = initial.selfPlayerId, life = initial.playerLifeSeq;
    const gunLevel = initial.localGameplayState.gunLevel;
    const state = async () => (await metrics(actor)).localGameplayState;
    assert.equal(initial.localGameplayState.abilities.multifireCapable, false);
    for (const key of ["-", "Delete"]) {
      await actor.press(key);
      assert.equal((await state()).abilities.multifire, false, "binding must not grant capability");
    }
    let target, claim;
    await waitUntil("natural fixture multifire spawn", async () => {
      const m = await metrics(actor);
      target = parseHiddenPrizeSpawns(server).filter(p => p.prizeId === 15)
        .sort((a, b) => Math.hypot(a.x-m.playerX, a.y-m.playerY)-Math.hypot(b.x-m.playerX, b.y-m.playerY))[0];
      return Boolean(target);
    }, 20_000);
    const route = await navigateAuthoredTarget(actor, target, server, { timeoutMs: 20_000, complete: () => {
      claim = parsePrizeClaims(server).find(p => p.playerId === actorId && p.collectibleId === target.collectibleId && p.prizeId === 15);
      return claim;
    } });
    assert.ok((route.reached || route.completed) && claim);
    assert.ok((await brakeClientVelocity(actor, 6000)).braked);
    await waitUntil("natural green grants capability without activation", async () => {
      const l = await state(); return l.abilities.multifireCapable && !l.abilities.multifire;
    }, 5000);
    stages.push({ label: "natural-acquisition", target, claim, route });
    async function toggle(key, enabled) {
      await actor.press(key);
      await waitUntil(`${key} toggles multifire ${enabled}`, async () => {
        const l = await state(); assert.equal(l.gunLevel, gunLevel); return l.abilities.multifireCapable && l.abilities.multifire === enabled;
      }, 3000);
      stages.push({ label: "binding", key, enabled });
    }
    for (const [key, enabled] of [["-", true], ["Delete", false], ["Delete", true], ["-", false]]) await toggle(key, enabled);
    await actor.press("Escape");
    await waitUntil("Escape menu owns the keyboard", async () => (await state()).menuVisible, 3000);
    await actor.press("-", 60, false); await actor.press("Delete", 60, false);
    const menuIsolated = await state();
    assert.ok(menuIsolated.menuVisible);
    assert.equal(menuIsolated.abilities.multifire, false);
    await actor.press("Escape", 60, false);
    await waitUntil("Escape returns to gameplay", async () => !(await state()).menuVisible, 3000);
    await actor.press("Enter");
    await waitUntil("Enter transfers keyboard ownership to chat", async () => (await metrics(actor)).chatActive, 3000);
    await actor.press("-", 60, false); await actor.press("Delete", 60, false);
    const chatIsolated = await metrics(actor);
    assert.equal(chatIsolated.localGameplayState.abilities.multifire, false);
    assert.ok(chatIsolated.chatActive, "chat must retain keyboard ownership");
    await actor.press("Escape", 60, false);
    await waitUntil("Escape closes only chat", async () => {
      const m = await metrics(actor); return !m.chatActive && !m.localGameplayState.menuVisible;
    }, 3000);
    stages.push({ label: "menu-chat-isolation", menuVisible: menuIsolated.menuVisible,
      menuMultifire: menuIsolated.abilities.multifire, chatActive: chatIsolated.chatActive,
      chatMultifire: chatIsolated.localGameplayState.abilities.multifire });
    const acquiredPose = await metrics(actor);
    const witnessApproach = await navigateAuthoredTarget(observer,
      { x: acquiredPose.playerX, y: acquiredPose.playerY }, server, { arrivalRadiusPx: 48 });
    stages.push({ label: "acquired-capability-witness-approach", ...witnessApproach });
    await waitForPlayersReady(actor, observer, server);
    const a = await metrics(actor), b = await metrics(observer);
    assert.ok((await orientClientForProtocolDirection(actor, a.playerX-b.playerX, a.playerY-b.playerY, 6000)).oriented,
      "aim away from observer before the firing-pattern audit");
    async function fire(label, enabled) {
      await waitUntil("full energy and recovered gun cooldown", async () => {
        const m = await metrics(actor), l = m.localGameplayState;
        return m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy && l.bulletCooldown <= 0;
      }, 10_000);
      const offset = server.logs.join("").length, sourceIndices = new Set();
      await actor.press("Control", 55);
      let spawned, observerFrames;
      await waitUntil(label, async () => {
        spawned = parseServerProjectileEvents(server.logs.join("").slice(offset)).filter(e => e.type === "server_projectile_spawned" && e.ownerId === actorId && e.ownerLifeSeq === life);
        if (!spawned.length) return false;
        const groups = new Set(spawned.map(e => e.shotGroupId));
        assert.equal(groups.size, 1, "one tap must admit exactly one shot group");
        assert.equal(spawned.length, enabled ? 3 : 1);
        assert.ok(spawned.every(e => Boolean(e.weapon & 0x8000) === enabled && ((e.weapon >> 5) & 3) + 1 === gunLevel));
        // Browser u64 group ids are decimal strings to preserve identity. Read
        // retained frames as well: a short-lived bullet may already hit a wall
        // by the time the test process finishes reading its server event.
        observerFrames = await observer.evaluate(`window.__zubspaceMetricsTrace.filter(f =>
          f.remoteProjectileSources?.some(p => p.playerId === ${actorId} && p.lifeSeq === ${life} && p.shotGroupId === ${JSON.stringify(String(spawned[0].shotGroupId))}))
          .map(f => ({renderOrdinal:f.renderOrdinal, at:f.at, sources:f.remoteProjectileSources.filter(p =>
            p.playerId === ${actorId} && p.lifeSeq === ${life} && p.shotGroupId === ${JSON.stringify(String(spawned[0].shotGroupId))})}))`);
        for (const frame of observerFrames) for (const p of frame.sources) {
          assert.ok(p.authenticated && p.kind === "bullet");
          assert.ok(spawned.some(e => e.projectileId === p.authorityProjectileId));
          sourceIndices.add(p.sourceProjectileIndex);
        }
        return sourceIndices.size === (enabled ? 3 : 1);
      }, 5000);
      screenshots[label] = await actor.screenshot();
      stages.push({ label, enabled, spawned, observerSourceIndices: [...sourceIndices], observerFrames });
      console.log(JSON.stringify({ stage: label, projectiles: spawned.length, observerSourceIndices: [...sourceIndices] }));
      await sleep(150);
      assert.equal(parseServerProjectileEvents(server.logs.join("").slice(offset)).filter(e => e.type === "server_projectile_spawned" && e.ownerId === actorId).length, enabled ? 3 : 1);
    }
    await fire("single-before", false);
    await toggle("-", true); await fire("multifire-on", true);
    await toggle("Delete", false); await fire("single-restored", false);
    assert.equal((await metrics(actor)).playerLifeSeq, life);
    assert.equal((await metrics(observer)).playerLifeSeq, observerInitial.playerLifeSeq);
    assert.equal((await state()).gunLevel, gunLevel);
    for (const client of clients) {
      const s = await client.pageState();
      assert.equal(s.uiMetrics.localGameplayState.rtcRejoinCount, 0);
      assert.ok(!s.networkLog.some(e => e.level === "error"));
      assert.ok(!client.events.some(e => ["error", "exception"].includes(e.type)));
    }
    console.log(JSON.stringify(await evidence.succeed({ ok: true, fixture, actorId,
      coverage: "natural multifire capability; minus/Delete aliases; menu/chat isolation; one/three/one projectile groups on server and authenticated observer; unchanged gun level/life/RTC", stages }, capture()), null, 2));
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
