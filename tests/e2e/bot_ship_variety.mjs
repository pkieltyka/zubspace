#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening, sleep } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { parseServerDamageEvents } from "../../tools/server_events.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, summarizeClient, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { botEnergyCapturePolicy, botEnergySustainedCaptureHint, botWeaponEnergyEvidence, nextBotEnergyCaptureAt } from "./bot_weapon_energy_evidence.mjs";
import { createBotShotReader } from "./spectator_bot_audio.mjs";
import { createBotSpawnFixture } from "./bot_spawn_fixture.mjs";
import { collectArtifactProvenance } from "./artifact_provenance.mjs";
import { assertQaServerBinaryProvenance, assertQaWasmProvenance } from "./qa_trace_artifacts.mjs";

const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const energyOnly = process.env.ZUBSPACE_E2E_BOT_WEAPON_ENERGY === "1";

// Ordinary moving/firing bots with stock Chaos physics, energy and weapons.
// An explicit spawn-only fixture can keep this focused encounter central;
// random-spawn acceptance remains separate. No weapon or lifecycle shortcuts.
async function main() {
  const identity = { e2eScript: "tests/e2e/bot_ship_variety.mjs",
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) };
  const evidence = createFocusedBrowserEvidence(identity);
  // Hash the build at the scenario boundary. Rehashing the whole worktree on
  // every marker consumes the small ring's overlap; the evidence finalizer
  // independently checks the end boundary against its pinned initial tree.
  const builtProvenance = collectArtifactProvenance(identity);
  // The 15-bot trace retains ~2s under its fixed byte cap. Keep that combat
  // matrix and use a separate three-bot lane for the complete >=3s debit/
  // recharge window. Both run in the combined Make gate.
  const pool = energyOnly ? [0, 4] : [0, 1, 3, 4, 6];
  const totalBots = energyOnly ? 3 : 15;
  const expected = new Map(Array.from({ length: totalBots }, (_, i) => [768+i, pool[i%pool.length]]));
  const settingsText = fs.readFileSync("assets/zones/chaos-zone/arena.conf", "utf8");
  const sections = new Map(); let section;
  for (const raw of settingsText.split("\n")) {
    const line = raw.trim();
    if (/^\[.+\]$/.test(line)) { section = line.slice(1, -1); sections.set(section, {}); }
    else if (/^\w+=\d+$/.test(line)) { const [key, value] = line.split("="); sections.get(section)[key] = Number(value); }
  }
  const shipNames = ["Warbird", "Javelin", "Spider", "Leviathan", "Terrier", "Weasel", "Lancaster", "Shark"];
  const profiles = new Map([...expected].map(([id, ship]) => {
    const cfg = { ...sections.get("AllShips"), ...sections.get(shipNames[ship]) };
    return [id, { ship, maxGuns: cfg.MaxGuns, bulletFireEnergy: cfg.BulletFireEnergy,
      bulletFireDelay: cfg.BulletFireDelay, rechargePerSecond: cfg.MaximumRecharge / 10,
      doubleBarrel: cfg.DoubleBarrel === 1, maxBombs: cfg.MaxBombs,
      bombFireEnergy: cfg.BombFireEnergy, bombFireEnergyUpgrade: cfg.BombFireEnergyUpgrade,
      bombFireDelay: cfg.BombFireDelay }];
  }));
  // Exercise the requested short alias against the shipped Warbird-only config.
  delete process.env.ZUBSPACE_SERVER_BOT_SHIP_TYPES;
  const botSpawnFixture = createBotSpawnFixture("chaos-zone", process.env.ZUBSPACE_E2E_BOT_SPAWN_FIXTURE || "");
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { botCount: 0, zoneRoot: botSpawnFixture?.root || null, env: {
    ...(botSpawnFixture ? { ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" } : {}),
    ZUBSPACE_SERVER_BOT_COUNT: String(totalBots), BOT_SHIP_TYPES: pool.map(s => s+1).join(","), ZUBSPACE_SERVER_LOG_FORMAT: "json",
    ZUBSPACE_SERVER_QA_TRACE_UPLOAD: "1",
    ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT: "true",
    ...(energyOnly ? { ZUBSPACE_SERVER_LOG_FILTER: "zubspace_server::server_tick::bots=debug" } : {}),
    ZUBSPACE_SERVER_SIGNALING_ALLOWED_ORIGINS: `http://127.0.0.1:${staticServer.port}`,
  } });
  const browser = startChrome(await freePort(), "bot-variety");
  const url = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&name=botwitness&trace=1&packetlog=1&password=secret`;
  let client, report;
  const screenshots = {}, appearances = new Map(), frequencies = new Map(), lifeSamples = [];
  const traceCaptures = [], traceEvents = new Map(), traceCaptureTimes = [], traceCaptureTriggers = [];
  const readBotShots = createBotShotReader(server.logs);
  let nextTraceCaptureAt = 0;
  const capture = () => ({ server, serverPort: port, staticServer, urls: [url],
    clients: client ? [{ label: "witness", client }] : [], extraScreenshots: screenshots,
    extraLogs: { "bot-spawn-fixture": JSON.stringify(botSpawnFixture?.evidence || null), appearances: JSON.stringify([...appearances], null, 2), "life-samples": JSON.stringify(lifeSamples, null, 2), "server-qa-captures": JSON.stringify(traceCaptures, null, 2), "capture-times": JSON.stringify(traceCaptureTimes), "capture-triggers": JSON.stringify(traceCaptureTriggers), "energy-profiles": JSON.stringify([...profiles]), "combat-report": JSON.stringify(report ?? null), "browser": browser.logs },
    disconnect: async () => { if (client) await client.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}); },
  });
  try {
    await waitForServerListening(port, server.child, server.logs); await waitForChrome(browser);
    client = await openChromeClient(browser, url); await client.enterShip();
    let reportedCaptures = 0, observationError = null;
    await waitUntil(energyOnly
      ? "configured ships participate with sustained exact energy accounting and a gun/gun/bomb sequence"
      : "all configured ship types draw and take part in authoritative combat", async () => {
      try {
        // The variety lane includes human counterfire. The energy witness
        // observes without shooting: killing its hunter would prematurely end
        // the very same-life sustained sequence this companion lane measures.
        // Surplus bots still provide ordinary bot-v-bot damage and respawns.
        if (energyOnly) await sleep(75);
        else await client.press("Control", 75);
        const state = (await client.metricsState()).uiMetrics;
        const frame = await client.evaluate("window.__zubspaceRemoteSpriteTrace?.at(-1)");
        for (const entry of frame?.entries || []) {
          if (!expected.has(entry.id)) continue;
          assert.equal(entry.ship, expected.get(entry.id), `ship assignment changed for bot ${entry.id}`);
          if (entry.drawn && !appearances.has(entry.ship)) {
            appearances.set(entry.ship, { ...entry, snapshotTick: state.latestSnapshotServerTick });
            screenshots[`ship-${entry.ship+1}`] = await client.screenshot();
          }
        }
        for (const player of state.remotePlayers || []) if (expected.has(player.id)) {
          assert.equal(player.frequency, player.id);
          frequencies.set(player.id, player.frequency);
        }
        const damage = parseServerDamageEvents(server.logs).filter(e => expected.has(e.shooterId) && e.damage > 0);
        const damaging = new Set(damage.map(e => expected.get(e.shooterId)));
        const botVsBot = damage.filter(e => expected.has(e.victimId) && e.victimId !== e.shooterId);
        for (const entry of frame?.entries || []) if (expected.has(entry.id) && entry.lifeSeq > 1) {
          if (!lifeSamples.some(e => e.id === entry.id && e.lifeSeq === entry.lifeSeq)) lifeSamples.push(entry);
        }
        const basic = pool.every(ship => appearances.has(ship) && damaging.has(ship))
          && frequencies.size === totalBots && botVsBot.length > 0 && lifeSamples.length > 0;
        // Bot spawn/respawn facts deliberately live in the bounded QA registry,
        // rather than noisy stdout. Read that real diagnostic surface at cold
        // checkpoints, respecting its four-uploads-per-minute limit.
        // Reserve one upload for an actual bomb instead of spending every
        // slot on timer samples and missing the later attack. Native logs
        // only trigger capture; the joined exact-energy QA facts below remain
        // the verdict. Do not manufacture a bomb or expand the server quota.
        const observedBomb = [...traceEvents.values()].some(e => e.type === "server_weapon_event_committed"
          && expected.has(e.ownerId) && [3, 4].includes(e.weapon & 31));
        const observedSustained = (report?.weaponEnergy?.sustained.length ?? 0) > 0;
        const nativeShots = energyOnly ? readBotShots().filter(shot => expected.has(shot.owner_id)) : [];
        const freshBombHint = energyOnly && !observedBomb && nativeShots.some(shot =>
          expected.has(shot.owner_id) && [3, 4].includes(shot.weapon & 31)
          && shot.server_tick >= state.latestSnapshotServerTick - 100);
        const retainedTicks = traceCaptures.at(-1)?.server.events.map(e => e.serverTick).filter(Number.isInteger) || [];
        const retainedWindowTicks = retainedTicks.length ? Math.max(...retainedTicks) - Math.min(...retainedTicks) : 0;
        const sustainedHint = energyOnly && !observedSustained
          ? botEnergySustainedCaptureHint(nativeShots, state.latestSnapshotServerTick, retainedWindowTicks) : null;
        const now = Date.now();
        const { reserve, timerDue, hintDue } = energyOnly
          ? botEnergyCapturePolicy({ capturedAt: traceCaptureTimes, now,
              nextTimerAt: nextTraceCaptureAt, observedBomb, freshBombHint, sustainedHint })
          : { reserve: 0, timerDue: now >= nextTraceCaptureAt, hintDue: false };
        const captureDue = timerDue || hintDue;
        if (basic && captureDue) {
          traceCaptureTriggers.push({ at: now, timerDue, freshBombHint, sustainedHint, reserve });
          const joined = await client.freezeQaTrace({ upload: true });
          traceCaptures.push(joined);
          // Completion is a conservative bound on the server's quota debit.
          traceCaptureTimes.push(Date.now());
          assert.equal(joined.schema, "zubspace-qa-trace-joined-v1");
          assert.equal(joined.scope.playerId, state.selfPlayerId);
          assert.deepEqual(joined.provenance.serverBinary, joined.browser.provenance.serverBinary);
          assertQaServerBinaryProvenance(joined.browser, builtProvenance);
          assertQaWasmProvenance(joined.browser, builtProvenance);
          for (const event of joined.server.events) traceEvents.set(JSON.stringify(event), event);
          const retainedTicks = joined.server.events.map(e => e.serverTick).filter(Number.isInteger);
          const retainedWindowTicks = retainedTicks.length ? Math.max(...retainedTicks) - Math.min(...retainedTicks) : 0;
          // Spread the four allowed uploads over a longer continuous window,
          // using the observed ring span while retaining a real overlap.
          nextTraceCaptureAt = energyOnly
            ? nextBotEnergyCaptureAt(traceCaptureTimes, Date.now(), retainedWindowTicks,
              Number(![...traceEvents.values()].some(e => e.type === "server_weapon_event_committed"
                && expected.has(e.ownerId) && [3, 4].includes(e.weapon & 31))))
            : Date.now() + 21_000;
        }
        const events = [...traceEvents.values()];
        const shots = events.filter(e => e.type === "server_projectile_spawned" && expected.has(e.ownerId));
        const fired = new Set(shots.map(e => expected.get(e.ownerId)));
        const respawns = events.filter(e => e.type === "server_actor_respawn_committed" && e.actorKind === "bot" && expected.has(e.playerId));
        const coverageWindows = traceCaptures.map(capture => {
          const ticks = capture.server.events.map(e => e.serverTick).filter(Number.isInteger);
          return [Math.min(...ticks), Math.max(...ticks)];
        });
        const weaponEnergy = botWeaponEnergyEvidence(events, profiles, { coverageWindows, requireSustained: energyOnly });
        if (traceCaptures.length > reportedCaptures) {
          reportedCaptures = traceCaptures.length;
          console.log(JSON.stringify({ stage: "bot-variety-trace", captures: reportedCaptures,
            drawn: [...appearances.keys()], fired: [...fired], damaging: [...damaging], respawns: respawns.length,
            captureWindow: coverageWindows.at(-1), nextCaptureDelayMs: Math.max(0, nextTraceCaptureAt - Date.now()),
            observedBombs: events.filter(e => e.type === "server_weapon_event_committed" && expected.has(e.ownerId) && [3, 4].includes(e.weapon & 31)).length,
            sustained: weaponEnergy?.sustained.length ?? 0, combos: weaponEnergy?.bulletBombSequences.length ?? 0 }));
        }
        report = { fired: [...fired], damaging: [...damaging], damage, botVsBot, respawns,
          shotCount: shots.length, tick: state.latestSnapshotServerTick, weaponEnergy };
        return pool.every(ship => appearances.has(ship) && fired.has(ship) && damaging.has(ship))
          && frequencies.size === totalBots && botVsBot.length > 0 && respawns.length > 0 && lifeSamples.length > 0
          && weaponEnergy != null && weaponEnergy.bombActivations > 0
          && (!energyOnly || weaponEnergy.bulletBombSequences.length > 0);
      } catch (error) {
        // waitUntil retries transient predicates. A contradictory accounting
        // or identity assertion is terminal and must keep its original cause.
        observationError = error;
        return true;
      }
    }, 90_000);
    if (observationError) throw observationError;
    const state = await client.artifactState();
    const roster = summarizeClient(state).rosters.filter(row => expected.has(row.id));
    const names = new Map();
    for (const row of roster) {
      assert.equal(row.ship, expected.get(row.id));
      assert.ok(row.name.length > 0 && row.name.length <= 14 && !row.name.startsWith("Bot "));
      if (names.has(row.id)) assert.equal(names.get(row.id), row.name, "roster updates must preserve names");
      names.set(row.id, row.name);
    }
    assert.equal(names.size, totalBots);
    assert.equal(new Set(names.values()).size, totalBots);
    // Hold the list over a roster longer than the viewport. Genuine arrows
    // must scroll it without changing facing or dispatching any weapons.
    // Use a short viewport to require scrolling the expanded table.
    if (!energyOnly) {
      await client.setViewport(1280, 160);
      await client.keyDown("Space");
      let list;
      try {
        await waitUntil("held Space opens player list", async () => {
          list = (await client.metricsState()).uiMetrics;
          return list.scoreboardVisible && list.scoreboardMaxScrollRow > 0;
        }, 3000);
        const before = list;
        screenshots["player-list-top"] = await client.screenshot();
        await client.press("ArrowDown", 1500);
        const down = (await client.metricsState()).uiMetrics;
        assert.equal(down.scoreboardScrollRow, down.scoreboardMaxScrollRow);
        assert.equal(down.localGameplayState.inputBits, 0);
        if (down.playerLifeSeq === before.playerLifeSeq) assert.equal(down.playerOrientation, before.playerOrientation);
        assert.deepEqual(down.semanticInputCounts, before.semanticInputCounts);
        screenshots["player-list-bottom"] = await client.screenshot();
        await client.press("ArrowUp", 1500);
        const up = (await client.metricsState()).uiMetrics;
        assert.equal(up.scoreboardScrollRow, 0);
        assert.deepEqual(up.semanticInputCounts, before.semanticInputCounts);
        report.playerList = { first: before.scoreboardScrollRow, bottom: down.scoreboardScrollRow,
          maximum: down.scoreboardMaxScrollRow, returned: up.scoreboardScrollRow };
      } finally { await client.keyUp("Space"); }
      await client.setViewport(1280, 720);
      await waitUntil("Space release closes player list", async () => !(await client.metricsState()).uiMetrics.scoreboardVisible, 3000);
    }
    if (energyOnly) assert.equal(state.uiMetrics.semanticInputCounts.primary, 0, "energy witness must not retire its autonomous source");
    assert.equal(state.uiMetrics.localBotCount, 0);
    assert.equal(state.uiMetrics.localGameplayState.rtcRejoinCount, 0);
    assert.equal(new Set(frequencies.values()).size, totalBots);
    assert.ok(!frequencies.has(state.uiMetrics.playerFrequency));
    assert.ok(!state.networkLog.some(e => e.level === "error"));
    assert.ok(!client.events.some(e => ["error", "exception"].includes(e.type)));
    screenshots.final = await client.screenshot();
    console.log(JSON.stringify(await evidence.succeed({ ok: true, botSpawnFixture: botSpawnFixture?.evidence || null, profile: energyOnly ? "sustained-energy" : "full-variety", totalBots, shipPool: pool.map(s => s+1),
      appearances: [...appearances], frequencies: [...frequencies], names: [...names], lifeSamples, combat: report,
      scope: "all configured ship types rendered, fired and dealt real damage; distinct bot frequencies; bot-v-bot damage and respawn retaining assigned ship; " +
        (energyOnly ? "complete sustained firing debit/recharge window" : "per-activation debit checks in retained windows; sustained observation requires the companion energy lane") }, capture()), null, 2));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await client?.close().catch(() => {});
    await Promise.all([stopProcess(browser.child), stopProcess(server.child)]).catch(() => {});
    fs.rmSync(browser.userDataDir, {recursive:true,force:true,maxRetries:10,retryDelay:150});
    if (botSpawnFixture) fs.rmSync(botSpawnFixture.root, {recursive:true,force:true});
    staticServer.server.close();
  }
}
main().catch(error => { console.error(error.stack || error); process.exit(1); });
