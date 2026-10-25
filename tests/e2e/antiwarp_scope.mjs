#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { freePort, sleep, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { parseZubspaceEvents } from "../../tools/server_events.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady, waitForLivePlayerScopes, rendezvousPlayers,
  parseHiddenPrizeSpawns, parsePrizeClaims, browserE2eFailureFooter } from "./combat_replay.mjs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";

// Shipped Chaos map/balance, a declared timed-pickup origin and reference
// starting loadouts (not the product maxed-spawn policy). Natural Antiwarp,
// genuine keys, no inventory/status grants or raw gameplay packets.
async function main() {
  // Seed 220 yields protocol prize 20 (Antiwarp) at (8232,8184) in
  // the first two-player batch. Prize 24 is Thor, not Antiwarp.
  process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED = "220";
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/antiwarp_scope.mjs" });
  const port = await freePort();
  const staticServer = await startStaticServer();
  // The first seeded green lives44s; production random spawns can require
  // an84s terrain detour. Declare this mechanic lane's origin, preserving
  // every map, balance, prize and timer field plus the genuine pickup proof.
  const fixture = { ...createSpawnRegionFixture({ zoneId: "chaos-zone",
    spawnRegion: { x: 512, y: 512, radius: 0 } }), maxedSpawnLoadout: false };
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root,
    env: { ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT: "false", ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" } });
  const browsers = [startChrome(await freePort(), "antiwarp-owner"), startChrome(await freePort(), "antiwarp-victim")];
  const base = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&trace=1&packetlog=1&password=secret`;
  const urls = [`${base}&name=jammer`, `${base}&name=warper`];
  const clients = [], stages = [], screenshots = {};
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: i ? "warper" : "jammer", client })),
    extraScreenshots: screenshots, extraLogs: { fixture: JSON.stringify(fixture, null, 2), stages: JSON.stringify(stages, null, 2),
      "scenario-settings": fs.readFileSync(path.join(fixture.root, "chaos-zone/arena.conf"), "utf8"),
      "scenario-zone-bundle": fs.readFileSync(path.join(fixture.root, "chaos-zone/zone.bundle"), "utf8"),
      "jammer-browser": browsers[0].logs, "warper-browser": browsers[1].logs },
    disconnect: async () => { await Promise.all(clients.map(client => client.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); },
  });
  const metrics = async client => (await client.metricsState()).uiMetrics;
  const events = () => parseZubspaceEvents(server.logs).map(({ event }) => event);
  try {
    await waitForServerListening(port, server.child, server.logs);
    const serverStatus = await (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
    assert.equal(serverStatus.maxed_spawn_loadout, false, "natural acquisition fixture requires reference starting loadouts");
    await Promise.all(browsers.map(waitForChrome));
    for (let i = 0; i < browsers.length; i++) {
      clients.push(await openChromeClient(browsers[i], urls[i]));
      await clients[i].enterShip();
    }
    const [jammer, warper] = clients;
    await waitForLivePlayerScopes(jammer, warper);
    await warper.press("ArrowRight", 70);
    const initial = await Promise.all(clients.map(metrics));
    const jammerId = initial[0].selfPlayerId, warperId = initial[1].selfPlayerId;
    const lives = initial.map(m => m.playerLifeSeq);
    assert.equal(initial[0].localGameplayState.abilities.antiwarpCapable, false);
    assert.equal(initial[1].localGameplayState.portals, 3);
    let target, claim;
    await waitUntil("natural authored Chaos Antiwarp spawn", () => {
      target = parseHiddenPrizeSpawns(server).find(p => p.prizeId === 20);
      return Boolean(target);
    }, 20_000);
    const route = await navigateAuthoredTarget(jammer, target, server, { complete: async () => {
      claim = parsePrizeClaims(server).find(p => p.playerId === jammerId && p.collectibleId === target.collectibleId && p.prizeId === 20);
      return claim;
    } });
    stages.push({ label: "natural-antiwarp-pickup-flight", target, claim: claim ?? null, route });
    assert.ok((route.reached || route.completed) && claim, "genuine flight must claim the natural Antiwarp green");
    await waitUntil("Antiwarp capability arrives privately without activating", async () => {
      const a = (await metrics(jammer)).localGameplayState.abilities;
      return a.antiwarpCapable && !a.antiwarp;
    }, 5_000);
    stages.push({ label: "natural-antiwarp-acquired", target, claim, route });
    await rendezvousPlayers(jammer, warper, server);
    await waitForPlayersReady(jammer, warper, server);
    async function checkLives() {
      const all = await Promise.all(clients.map(metrics));
      all.forEach((m, i) => assert.equal(m.playerLifeSeq, lives[i], "ability audit must stay on the original life"));
      return all;
    }
    async function antiwarp(enabled) {
      await jammer.press("9");
      await waitUntil(`Antiwarp ${enabled ? "on" : "off"} reaches opponent`, async () => {
        const [a, b] = await checkLives();
        const remote = b.remoteActorPresentation.find(p => p.id === jammerId);
        return a.localGameplayState.abilities.antiwarp === enabled && remote && Boolean(remote.togglables & 8) === enabled;
      }, 5_000);
    }
    async function place(label) {
      const before = await metrics(warper), offset = events().length;
      await warper.press("0");
      let terminal;
      await waitUntil(label, async () => {
        terminal = events().slice(offset).find(e => e.type === "server_portal_placement_terminal" && e.player_id === warperId);
        const m = await metrics(warper), l = m.localGameplayState;
        return terminal?.outcome === 1 && l.portalActive && l.pendingPortalPlacements === 0
          && l.portals === before.localGameplayState.portals - 1 && l.prizeAuthorityRevision === terminal.prize_state_revision;
      }, 6_000);
      stages.push({ label, terminal });
      assert.equal(terminal.expires_at_tick - terminal.server_tick, 2000,
        "authored Chaos portal lifetime is twenty seconds");
      return terminal;
    }
    async function blocked(label, random, portalExists) {
      const [, before] = await checkLives(), offset = events().length;
      const distance = Math.hypot(before.playerX - (await metrics(jammer)).playerX, before.playerY - (await metrics(jammer)).playerY);
      assert.ok(distance < 640, "actors must be inside authored AntiWarpPixels");
      if (random) await warper.chord("Shift", "0"); else await warper.press("0");
      const field = portalExists || random ? "warp" : "portal";
      await waitUntil(`${label}: genuine semantic key reached gameplay`, async () =>
        (await metrics(warper)).semanticInputCounts[field] === before.semanticInputCounts[field] + 1, 3_000);
      const until = Date.now() + 1200;
      while (Date.now() < until) {
        const [a, b] = await checkLives(), l = b.localGameplayState;
        assert.ok(a.localGameplayState.abilities.antiwarp);
        assert.equal(l.portalActive, portalExists);
        assert.equal(l.portals, before.localGameplayState.portals);
        assert.equal(l.pendingPortalPlacements, 0);
        assert.equal(l.pendingPortalWarp, false);
        assert.ok(!events().slice(offset).some(e => e.player_id === warperId && /server_portal_(placement|warp)_terminal/.test(e.type)), "client must gate requests while it observes enemy Antiwarp");
        assert.ok(Math.hypot(b.playerX - before.playerX, b.playerY - before.playerY) < 20, "blocked key must not relocate the actor");
        await sleep(100);
      }
      screenshots[label] = await warper.screenshot();
      stages.push({ label, distance, semanticAction: field, snapshot: await metrics(warper) });
      console.log(JSON.stringify({ stage: label, ok: true }));
    }
    await place("portal-placed-before-jamming");
    await antiwarp(true);
    await blocked("recall-blocked", false, true);
    await blocked("random-blocked", true, true);
    await antiwarp(false);
    let offset = events().length;
    await warper.press("0");
    let recall;
    await waitUntil("deactivation releases authoritative portal recall", async () => {
      recall = events().slice(offset).find(e => e.type === "server_portal_warp_terminal" && e.player_id === warperId);
      const l = (await metrics(warper)).localGameplayState;
      return recall?.outcome === 1 && recall.used_portal && !l.portalActive && !l.pendingPortalWarp && l.fakeAntiwarpTimer <= 0;
    }, 6_000);
    stages.push({ label: "recall-released", terminal: recall });
    await antiwarp(true);
    await blocked("placement-blocked", false, false);
    await antiwarp(false);
    const retainedPortal = await place("placement-released");
    await waitUntil("full authoritative energy for random warp", async () => {
      const m = await metrics(warper);
      return m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy;
    }, 25_000);
    offset = events().length;
    await warper.chord("Shift", "0");
    let random;
    await waitUntil("deactivation releases random warp and preserves endpoint", async () => {
      random = events().slice(offset).find(e => e.type === "server_portal_warp_terminal" && e.player_id === warperId);
      const l = (await metrics(warper)).localGameplayState;
      return random?.outcome === 1 && !random.used_portal && l.portalActive && !l.pendingPortalWarp;
    }, 6_000);
    stages.push({ label: "random-released", terminal: random });

    async function expiry(label, terminal) {
      const before = await metrics(warper), samples = [];
      assert.ok(before.localGameplayState.portalActive, "expiry must begin with a live portal");
      await waitUntil(label, async () => {
        const [, m] = await checkLives(), l = m.localGameplayState;
        samples.push({ tick: m.latestSnapshotServerTick, active: l.portalActive,
          ttl: l.portalTtl, age: l.portalAge, inventory: l.portals });
        assert.equal(l.portals, before.localGameplayState.portals, "expiry must not restore or spend inventory");
        // Leave two snapshots of clock tolerance at the exact transition;
        // well before that boundary the endpoint must still exist.
        if (m.latestSnapshotServerTick < terminal.expires_at_tick - 20) assert.ok(l.portalActive, "portal expired before its authored deadline");
        return m.latestSnapshotServerTick >= terminal.expires_at_tick && !l.portalActive;
      }, 25_000);
      assert.ok(samples.some(s => s.active), "retain the active side of natural expiry");
      assert.ok(samples.at(-1).tick >= terminal.expires_at_tick);
      stages.push({ label, terminal, samples });
      screenshots[label] = await warper.screenshot();
      console.log(JSON.stringify({ stage: label, deadline: terminal.expires_at_tick,
        observedExpiredTick: samples.at(-1).tick }));
    }
    // Wait the real authored lifetime; no shortened timer or runtime hook.
    await expiry("retained-portal-expires", retainedPortal);
    const replacement = await place("zero-replaces-expired-endpoint");
    assert.equal(replacement.portals_remaining, 0, "the third genuine placement spends the final starting portal");
    await expiry("replacement-portal-expires", replacement);
    const beforeEmpty = await metrics(warper), emptyOffset = events().length;
    await warper.press("0");
    await waitUntil("empty zero is a placement attempt, never a random warp", async () =>
      (await metrics(warper)).semanticInputCounts.portal === beforeEmpty.semanticInputCounts.portal + 1, 3000);
    const emptyUntil = Date.now() + 1200;
    while (Date.now() < emptyUntil) {
      const [, m] = await checkLives(), l = m.localGameplayState;
      assert.equal(l.portals, 0);
      assert.equal(l.portalActive, false);
      assert.equal(l.pendingPortalPlacements, 0);
      assert.equal(l.pendingPortalWarp, false);
      assert.ok(Math.hypot(m.playerX-beforeEmpty.playerX, m.playerY-beforeEmpty.playerY) < 2,
        "unavailable placement must not silently random-warp");
      assert.ok(!events().slice(emptyOffset).some(e => e.player_id === warperId && /server_portal_(placement|warp)_terminal/.test(e.type)));
      await sleep(100);
    }
    stages.push({ label: "empty-zero-does-not-fallback", snapshot: await metrics(warper) });
    await waitUntil("full energy for random warp without any portal", async () => {
      const m = await metrics(warper); return m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy;
    }, 25_000);
    offset = events().length;
    await warper.chord("Shift", "0");
    let withoutPortal;
    await waitUntil("Shift-zero random warp succeeds without an endpoint", async () => {
      withoutPortal = events().slice(offset).find(e => e.type === "server_portal_warp_terminal" && e.player_id === warperId);
      const l = (await metrics(warper)).localGameplayState;
      return withoutPortal?.outcome === 1 && !withoutPortal.used_portal && !l.portalActive && !l.pendingPortalWarp && l.portals === 0;
    }, 6000);
    stages.push({ label: "random-without-portal", terminal: withoutPortal });
    await checkLives();
    for (const client of clients) {
      const state = await client.pageState();
      assert.equal(state.uiMetrics.localGameplayState.rtcRejoinCount, 0);
      assert.ok(!state.networkLog.some(e => e.level === "error"));
      assert.ok(!client.events.some(e => ["error", "exception"].includes(e.type)));
    }
    const result = await evidence.succeed({ ok: true, arena: "chaos-zone", jammerId, warperId,
      coverage: "natural Antiwarp acquisition; nearby placement/recall/random inhibition and release; real twenty-second portal expiry/replacement; empty zero has no fallback; Shift-zero random with/without endpoint; original life and RTC scope", stages }, capture());
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    error.message += `\n${browserE2eFailureFooter(artifacts)}`;
    throw error;
  } finally {
    await Promise.all(clients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    for (const b of browsers) fs.rmSync(b.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
    fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
