#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { repoRoot, freePort, startRealServer, startStaticServer, stopProcess, sleep,
  waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, waitForLivePlayerScopes,
  waitForPlayersReady, rendezvousPlayers, mergedPacketLog, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { browserConsoleEvidence, assertRendererConsoleClean } from "../../tools/browser_console_evidence.mjs";

// One small stock-Chaos fragment: initial random poses, genuine rendezvous,
// and a ship-change application receipt delayed without changing its bytes.
// Combat/death/Firefox coverage remains in the larger accumulated gates.
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/spawn_rendezvous.mjs" });
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port);
  const browsers = [startChrome(await freePort(), "spawn-alpha"), startChrome(await freePort(), "spawn-beta")];
  const clients = [], urls = [], screenshots = {}, samples = [];
  let report = null;
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: clients.map((client, index) => ({ label: index ? "beta" : "alpha", client })),
    extraScreenshots: screenshots,
    extraLogs: { "alpha-browser": browsers[0].logs, "beta-browser": browsers[1].logs,
      "spawn-observation": JSON.stringify({ report, samples }) },
    disconnect: async () => { await Promise.all(clients.map(client =>
      client.evaluate("(window.__zubspaceSendDisconnectForTest?.(),true)").catch(() => {}))); } });
  try {
    await waitForServerListening(port, server.child, server.logs);
    await Promise.all(browsers.map(waitForChrome));
    const offer = `http://127.0.0.1:${port}/v1/rtc/offer`;
    for (let index = 0; index < browsers.length; index++) {
      const url = `http://127.0.0.1:${staticServer.port}/?rtc=${encodeURIComponent(offer)}` +
        `&arena=chaos-zone&name=Spawn${index ? "Beta" : "Alpha"}&password=secret&trace=1&packetlog=1&lifecycle_receipt_impairment=armed`;
      urls.push(url);
      clients.push(await openChromeClient(browsers[index], url));
      await clients[index].enterShip();
    }
    const [alpha, beta] = clients;
    await waitForLivePlayerScopes(alpha, beta);
    const initial = await Promise.all(clients.map(async client => (await client.metricsState()).uiMetrics));
    const initialRendezvous = await rendezvousPlayers(alpha, beta, server);
    await waitForPlayersReady(alpha, beta, server);
    const before = (await alpha.metricsState()).uiMetrics;
    const target = (before.playerShip + 1) % 8;
    assert.equal(await alpha.evaluate("window.__zubspaceArmLifecycleReceiptImpairmentForTest?.() === true"), true);
    await alpha.press("Escape", 60);
    await alpha.press(String(target + 1), 60);
    await waitUntil("ship reset has an actual held application receipt", async () => {
      const impairment = await alpha.evaluate("window.__zubspaceLifecycleReceiptImpairmentStateForTest?.()");
      const metrics = (await alpha.metricsState()).uiMetrics;
      return impairment?.buffered === 1 && metrics.localRenderPrediction?.spawnPosePending === true &&
        metrics.localGameplayState?.authorityPhase === "AwaitingApplicationReceipt";
    }, 12_000);
    const until = Date.now() + 500;
    do {
      const metrics = (await alpha.metricsState()).uiMetrics;
      samples.push({ at: Date.now(), canonicalX: metrics.playerX, canonicalY: metrics.playerY,
        render: metrics.localRenderPrediction, life: metrics.playerLifeSeq });
      await sleep(35);
    } while (Date.now() < until);
    assert(samples.length >= 3, "hold needs repeated actual browser observations");
    const held = samples[0].render;
    for (const sample of samples) {
      assert.equal(sample.render.spawnPosePending, true);
      assert.equal(sample.render.shipPresentable, false);
      assert.equal(sample.render.x, held.x); assert.equal(sample.render.y, held.y);
      assert.equal(sample.render.inputBits, 0); assert.equal(sample.render.partialSeconds, 0);
      assert.equal(sample.canonicalX, 8200); assert.equal(sample.canonicalY, 8200);
    }
    assert(Math.hypot(held.x - 8200, held.y - 8200) > 1,
      "this capture must distinguish the held view from the neutral centre");
    screenshots.held = await alpha.screenshot();
    assert.equal(await alpha.evaluate("window.__zubspaceReleaseLifecycleReceiptImpairmentForTest?.() === true"), true);
    await waitForLivePlayerScopes(alpha, beta);
    const applied = (await alpha.metricsState()).uiMetrics;
    assert.notEqual(applied.playerLifeSeq, before.playerLifeSeq);
    assert.equal(applied.playerShip, target);
    assert.equal(applied.localRenderPrediction.spawnPosePending, false);
    assert.equal(applied.localRenderPrediction.shipPresentable, true);
    const state = await alpha.pageState();
    const packets = mergedPacketLog(state).filter(packet => packet.direction === "received" &&
      packet.bytes?.[0] === 0 && packet.bytes?.[1] === 3 && packet.bytes?.[6] === 0x75 && packet.bytes?.[7] === 3);
    const receipt = packets.map(packet => Buffer.from(packet.bytes)).findLast(bytes =>
      bytes.length === 26 && bytes.readUInt32LE(12) === applied.playerLifeSeq);
    assert(receipt, "exact successor v3 receipt must be retained");
    assert.equal(applied.playerX, receipt.readUInt16LE(22));
    assert.equal(applied.playerY, receipt.readUInt16LE(24));
    const replacementRendezvous = await rendezvousPlayers(alpha, beta, server);
    await waitForPlayersReady(alpha, beta, server);
    screenshots.recovered = await alpha.screenshot();
    const consoles = clients.map(client => browserConsoleEvidence(client));
    consoles.forEach(assertRendererConsoleClean);
    for (const client of clients) assert.equal(client.events.filter(event =>
      ["error", "exception"].includes(event.type)).length, 0, "complete browser console must have no errors");
    report = { initial: initial.map(m => ({ x: m.playerX, y: m.playerY, life: m.playerLifeSeq })),
      initialRendezvous, replacementRendezvous, heldView: held,
      applied: { x: applied.playerX, y: applied.playerY, life: applied.playerLifeSeq, ship: applied.playerShip },
      receiptHex: receipt.toString("hex"), consoles };
    const result = await evidence.succeed({ ok: true, arena: "chaos-zone", report,
      scope: "two headed Chrome pilots, stock random FFA spawns, genuine-key rendezvous, exact delayed ship-change receipt and held view; no combat/death/Firefox/full-acceptance claim" }, capture());
    console.log(JSON.stringify({ ok: result.ok, heldSamples: samples.length,
      initialDistancePx: Math.hypot(initial[0].playerX - initial[1].playerX, initial[0].playerY - initial[1].playerY),
      applied: report.applied, artifacts: result.successArtifacts }));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    await Promise.all(clients.map(client => client.close().catch(() => {})));
    await Promise.all([...browsers.map(browser => stopProcess(browser.child)), stopProcess(server.child)]).catch(() => {});
    for (const browser of browsers) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
