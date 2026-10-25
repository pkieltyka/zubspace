#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess,
  waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForPlayersReady,
  fireUntilDamage, waitForRespawnCycle, parseServerDamageEvents,
  browserE2eFailureFooter } from "./combat_replay.mjs";
import { parseServerDeathEvents } from "../../tools/server_events.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";

const appRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const artifactRoot = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR;
assert.ok(artifactRoot, "retain both live-score and reconnect evidence under an explicit directory");
// Login names use the game's validated character set; '-' is not accepted.
const names = ["ScoreAce", "ScoreHyde", "LatePilot"];
const metrics = async client => (await client.metricsState()).uiMetrics;

async function main() {
  // Stationary repeated kills require collocation after every genuine respawn.
  // Keep the fixture explicit; shipping spawns remain random.
  const fixture = createWeaponLifecycleFixture({ spawnRegion: { x: 512, y: 512, radius: 0 } });
  const port = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root, env: {
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_LOG_FORMAT: "json" } });
  const browsers = [], allClients = [], stages = [], screenshots = {};
  let clients = [], urls = [], evidence;
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, i) => ({ label: names[i], client })),
    extraScreenshots: screenshots,
    extraLogs: { fixture: JSON.stringify(fixture, null, 2), stages: JSON.stringify(stages, null, 2),
      ...Object.fromEntries(browsers.map((b, i) => [`browser-${i}`, b.logs])) },
    disconnect: async () => { await Promise.all(clients.map(c => c.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); } });
  const newEvidence = phase => createFocusedBrowserEvidence({
    e2eScript: "tests/e2e/expanded_scoring.mjs", artifactDir: path.join(artifactRoot, phase),
    wasm: process.env.ZUBSPACE_E2E_STATIC_ROOT ? path.relative(repoRoot, path.join(appRoot, "zubspace.wasm")) : undefined,
    clientEntry: path.relative(repoRoot, path.join(appRoot, "index.html")) });
  const identity = c => c.evaluate("JSON.parse(localStorage.getItem('zubspace.identity.v1'))?.playerId");
  async function join(i) {
    const url = `http://127.0.0.1:${staticServer.port}/?arena=chaos-zone&password=secret&trace=1&packetlog=1&name=${encodeURIComponent(names[i])}`;
    urls.push(url);
    const c = await openChromeClient(browsers[i], url);
    allClients.push(c); clients.push(c);
    await c.enterShip();
    return c;
  }
  async function expectRows(label, expected) {
    let observed;
    await waitUntil(label, async () => {
      observed = await Promise.all(clients.map(async c => {
        const m = await metrics(c);
        return { self: m.selfPlayerId, rows: m.scoreboardPlayers };
      }));
      return observed.every(({ rows }) => Array.isArray(rows) &&
        rows.length === Object.keys(expected).length && Object.entries(expected).every(([name, stats]) => {
          const row = rows.find(r => r.name === name);
          return row && Object.entries(stats).every(([key, value]) => row[key] === value);
        }));
    }, 10_000);
    stages.push({ label, expected: structuredClone(expected), observed });
  }
  async function tableCaptures(label) {
    for (let i = 0; i < clients.length; i++) {
      await clients[i].keyDown("Space");
      try {
        await waitUntil("expanded table is held", async () => (await metrics(clients[i])).scoreboardVisible, 3000);
        screenshots[`${label}-${names[i]}`] = await clients[i].screenshot();
      } finally { await clients[i].keyUp("Space"); }
      await waitUntil("table hides on release", async () => !(await metrics(clients[i])).scoreboardVisible, 3000);
    }
  }
  const expected = Object.fromEntries(names.slice(0, 2).map(name => [name,
    { score: 0, killPoints: 0, kills: 0, deaths: 0, averageBounty: 0, rating: 0 }]));
  try {
    evidence = newEvidence("live");
    await waitForServerListening(port, server.child, server.logs);
    for (let i = 0; i < 2; i++) { browsers.push(startChrome(await freePort(), names[i])); await waitForChrome(browsers[i]); }
    await join(0); await join(1);
    await waitForPlayersReady(...clients, server);
    const identities = await Promise.all(clients.map(identity));
    assert.equal(new Set(identities).size, 2, "the pilots need distinct persistent identities");
    const ids = await Promise.all(clients.map(async c => (await metrics(c)).selfPlayerId));
    await expectRows("fresh authoritative roster is zero", expected);
    for (const [cycle, shooterIndex] of [0, 0, 1].entries()) {
      const victimIndex = 1 - shooterIndex;
      const before = await metrics(clients[victimIndex]);
      const shooterBefore = await metrics(clients[shooterIndex]);
      assert.ok(Math.hypot(before.playerX - shooterBefore.playerX, before.playerY - shooterBefore.playerY) <= 4,
        `score cycle ${cycle + 1} requires the declared collocated stationary origin`);
      const deathCount = parseServerDeathEvents(server.logs).length;
      const kill = await fireUntilDamage({ label: `score cycle ${cycle + 1}`, shooter: clients[shooterIndex],
        victim: clients[victimIndex], server, shooterId: ids[shooterIndex], victimId: ids[victimIndex],
        sinceCount: parseServerDamageEvents(server.logs).length, requireLethal: true, singlePressMs: 60 });
      await waitUntil("committed server death has bounty", async () => parseServerDeathEvents(server.logs).length > deathCount, 3000);
      const deaths = parseServerDeathEvents(server.logs);
      assert.equal(deaths.length, deathCount + 1, "one kill must commit exactly one server death");
      const death = deaths.at(-1);
      assert.equal(death.damageSeq, kill.event.damageSeq);
      assert.equal(death.killerId, ids[shooterIndex]);
      assert.equal(death.victimId, ids[victimIndex]);
      assert.ok(death.killedBounty >= 30, "maximum loadout must retain the intended starting bounty");
      const won = expected[names[shooterIndex]], lost = expected[names[victimIndex]];
      won.kills += 1; won.killPoints += death.killedBounty; won.score = won.killPoints; lost.deaths += 1;
      for (const row of Object.values(expected)) {
        row.averageBounty = Number((row.killPoints / (row.kills || 1)).toFixed(1));
        row.rating = Math.max(0, Math.trunc((row.killPoints + (row.kills - row.deaths) * 10) * 10 / (row.kills + 100)));
      }
      await expectRows(`cycle ${cycle + 1}: exact server bounty points/counts on both clients`, expected);
      await waitForRespawnCycle(clients[victimIndex], before.playerRespawnSeq);
      await expectRows(`cycle ${cycle + 1}: totals survive respawn`, expected);
      stages.push({ label: `authoritative death ${cycle + 1}`, death, damage: kill.event });
    }
    await tableCaptures("live");
    await evidence.succeed({ ok: true, phase: "live", identities, ids, expected, stages,
      coverage: "three genuine-key committed kills; exact server bounty score, kills/deaths, Ave and rating agree on both rendered tables through respawn" }, capture());
    // The first phase captures both original sessions and then drains them.
    // Fresh tabs keep each browser's normal persistent identity, but establish
    // new RTC sessions; the second audit never excuses an unexpected dropout.
    for (const c of clients) await c.close();
    clients = []; urls = [];
    evidence = newEvidence("reconnect");
    await join(0); await join(1);
    assert.deepEqual(await Promise.all(clients.map(identity)), identities);
    await waitForPlayersReady(...clients, server);
    await expectRows("fresh RTC sessions restore both identity-scoped totals", expected);
    browsers.push(startChrome(await freePort(), names[2])); await waitForChrome(browsers[2]);
    await join(2);
    assert.ok(!identities.includes(await identity(clients[2])), "late join must have a fresh identity");
    expected[names[2]] = { score: 0, killPoints: 0, kills: 0, deaths: 0, averageBounty: 0, rating: 0 };
    await expectRows("late join roster agrees with both reconnected pilots", expected);
    await tableCaptures("reconnect");
    await evidence.succeed({ ok: true, phase: "reconnect", identities, expected, stages,
      coverage: "new RTC sessions recover both pilots' in-memory totals; a fresh identity starts at zero and sees the same full late-join roster" }, capture());
    console.log(JSON.stringify({ ok: true, artifactRoot, phases: ["live", "reconnect"], expected }));
  } catch (error) {
    if (evidence) {
      const a = await evidence.fail(error, capture());
      console.error(browserE2eFailureFooter(a));
    }
    throw error;
  } finally {
    await Promise.all(allClients.map(c => c.close().catch(() => {})));
    await Promise.all([...browsers.map(b => stopProcess(b.child)), stopProcess(server.child)]).catch(() => {});
    await new Promise(resolve => staticServer.server.close(resolve));
    fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
