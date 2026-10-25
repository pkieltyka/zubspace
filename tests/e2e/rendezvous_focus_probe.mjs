#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import { freePort, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, rendezvousPlayers, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

// Short genuine two-Chrome setup proof. No combat/prize-effect acceptance.
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/rendezvous_focus_probe.mjs" });
  const fixture = createSpawnRegionFixture({ zoneId: "chaos-zone", spawnRegion: { x: 512, y: 512, radius: 2 } });
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { botCount: 0, zoneRoot: fixture.root, env: {
    ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1", ZUBSPACE_SERVER_TEST_TEAM_MODE: "1", ZUBSPACE_HIDDEN_PRIZE_RNG_SEED: "20",
  } });
  const browsers = [startChrome(await freePort(), "focus-alpha"), startChrome(await freePort(), "focus-beta")];
  const clients = [], urls = [], routes = []; let before, after;
  const keyState = client => client.evaluate("window.__zubspaceGameKeyStateForTest()");
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, index) => ({ label: index ? "beta" : "alpha", client })),
    extraLogs: { fixture: JSON.stringify(fixture), routes: JSON.stringify(routes),
      focus: JSON.stringify({ before, after }), ...Object.fromEntries(browsers.map((browser, index) => [`${index}-chrome`, browser.logs])) },
    disconnect: async () => { await Promise.all(clients.map(client => client.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}))); } });
  try {
    await waitForServerListening(port, server.child, server.logs); await Promise.all(browsers.map(waitForChrome));
    for (const [index, browser] of browsers.entries()) {
      const url = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&name=FocusPilot${index}&password=secret&trace=1&packetlog=1`;
      urls.push(url); const client = await openChromeClient(browser, url); clients.push(client); await client.enterShip();
    }
    before = await Promise.all(clients.map(keyState));
    routes.push(await rendezvousPlayers(...clients, server));
    routes.push(await rendezvousPlayers(...clients, server, {
      stageRadiusPx: 20, meetingPoint: { x: 8248, y: 8168 },
    }));
    after = await Promise.all(clients.map(keyState));
    for (const [index, state] of after.entries()) {
      assert.equal(state.forcedReleaseBatches, before[index].forcedReleaseBatches,
        "joint controllers must not blur-release each other's held keys");
      assert.ok(state.keyDownEvents > before[index].keyDownEvents);
      assert.deepEqual(state.trackedCodes, []);
    }
    for (const route of routes) {
      assert.equal(route.status, "complete"); assert.ok(route.separationPx <= 24);
      assert.deepEqual(route.inputFocusIsolation.leasedActors, [0, 1]);
      assert.equal(route.jointArrival.length, 2);
      assert.ok(route.jointArrival.every(pose => Math.hypot(pose.velocityX, pose.velocityY) <= 3.5
        && Math.hypot(pose.x - route.plan.target.x, pose.y - route.plan.target.y) <= 4));
      assert.ok(route.final.every(pose => Math.hypot(pose.velocityX, pose.velocityY) <= 3.5));
    }
    console.log(JSON.stringify(await evidence.succeed({ ok: true, diagnosticOnly: true, routes, before, after,
      scope: "short joint real-key rendezvous over retained near-prize stage; original4px/rest3.5/24px separation and no automation-induced blur releases; not combat/effect/full-suite proof" }, capture())));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture()); console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await Promise.all(clients.map(client => client.close().catch(() => {})));
    await Promise.all([...browsers.map(browser => stopProcess(browser.child)), stopProcess(server.child)]);
    for (const browser of browsers) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    await new Promise(resolve => staticServer.server.close(resolve)); fs.rmSync(fixture.root, { recursive: true, force: true });
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
