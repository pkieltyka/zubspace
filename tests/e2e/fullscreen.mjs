#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { createSession, deleteSession, firefoxArtifactClient, freePort,
  holdFirefoxKeys, metricsState, openClient, screenshot, sleep, startGeckodriver,
  startRealServer, startStaticServer, stopProcess, waitForGeckodriver,
  waitForServerListening } from '../../tools/browser_firefox_rtc_smoke.mjs';
import { openChromeClient, startChrome, waitForChrome, waitUntil,
  browserE2eFailureFooter } from './combat_replay.mjs';
import { createFocusedBrowserEvidence } from './focused_browser_evidence.mjs';

const browser = process.env.ZUBSPACE_E2E_BROWSER || 'chrome';
assert.ok(['chrome', 'firefox'].includes(browser));
async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: 'tests/e2e/fullscreen.mjs' });
  const port = await freePort();
  const staticServer = await startStaticServer({ rtcOfferUrl: `http://127.0.0.1:${port}/v1/rtc/offer` });
  const server = startRealServer(port, { botCount: '0', env: { ZUBSPACE_SERVER_LOG_FORMAT: 'json' } });
  const driver = browser === 'chrome' ? startChrome(await freePort(), 'fullscreen') : startGeckodriver(await freePort(), 'fullscreen');
  const url = `http://127.0.0.1:${staticServer.port}/?trace=1&packetlog=1&arena=chaos-zone&name=fullscreen&password=secret`;
  let actor = null;
  const checkpoints = [], screenshots = {};
  const read = async () => (await actor.metricsState()).uiMetrics;
  const isFullscreen = () => actor.evaluate('document.fullscreenElement === document.documentElement');
  const checkpoint = async label => checkpoints.push({ label, fullscreen: await isFullscreen(), state: await read() });
  const capture = () => ({ server, serverPort: port, staticServer, urls: [url],
    clients: actor ? [{ label: browser, client: actor }] : [], extraScreenshots: screenshots,
    extraLogs: { checkpoints: JSON.stringify(checkpoints), browser: driver.logs },
    disconnect: async () => { await actor?.evaluate('(window.__zubspaceSendDisconnectForTest?.(), true)').catch(() => {}); },
  });
  const menu = async () => {
    if (!(await read()).localGameplayState.menuVisible) await actor.press('Escape');
    await waitUntil('Escape menu opens', async () => (await read()).localGameplayState.menuVisible, 3000);
  };
  const toggle = async expected => {
    await menu();
    await actor.press('f');
    await waitUntil(`fullscreen becomes ${expected} and menu closes`, async () =>
      (await isFullscreen()) === expected && !(await read()).localGameplayState.menuVisible, 5000);
  };
  try {
    await waitForServerListening(port, server.child, server.logs);
    if (browser === 'chrome') {
      await waitForChrome(driver); actor = await openChromeClient(driver, url);
    } else {
      await waitForGeckodriver(driver); await createSession(driver); await openClient(driver, url);
      actor = { ...firefoxArtifactClient(driver), metricsState: () => metricsState(driver),
        press: (key, ms = 100) => holdFirefoxKeys(driver, [key === 'Enter' ? '\uE006' : key], ms),
        screenshot: async () => Buffer.from(await screenshot(driver), 'base64') };
    }
    await waitUntil('live player', async () => (await read()).localGameplayState.authorityPhase === 'Live', 30000);
    await actor.press('ArrowUp', 250);
    await actor.press('f'); assert.equal(await isFullscreen(), false, 'F is inactive during gameplay');
    await actor.press('Enter'); await waitUntil('chat opens', async () => (await read()).chatActive, 3000);
    await actor.press('f'); assert.equal(await isFullscreen(), false, 'chat owns F');
    assert.equal((await read()).chatActive, true);
    await actor.press('Escape'); await waitUntil('chat closes', async () => !(await read()).chatActive, 3000);
    await toggle(true); await checkpoint('fullscreen-entered-menu-closed');
    screenshots.fullscreen = await actor.screenshot();
    // The browser owns Escape's native fullscreen-exit behavior.
    await actor.press('Escape');
    await sleep(350);
    const escapeExited = !(await isFullscreen());
    await checkpoint('browser-escape');
    assert.equal(escapeExited, true, 'Escape exits browser fullscreen');
    await toggle(true);
    await actor.press('Escape'); await sleep(350);
    assert.equal(await isFullscreen(), false, 'Escape exits fullscreen again');
    if ((await read()).localGameplayState.menuVisible) await actor.press('Escape');
    assert.equal(await isFullscreen(), false);
    assert.equal((await read()).localGameplayState.menuVisible, false);
    await checkpoint('fullscreen-toggled-back');
    const before = await read(); await actor.press('ArrowUp', 500); const after = await read();
    assert.ok(Math.hypot(after.playerX-before.playerX, after.playerY-before.playerY)>3, 'movement resumes after fullscreen');
    screenshots.final = await actor.screenshot();
    console.log(JSON.stringify(await evidence.succeed({ ok:true, browser, escapeExited,
      coverage:'real-key gameplay/chat isolation, full-page entry, automatic menu close, repeat entry/exit and movement' },capture())));
  } catch (error) {
    console.error(error.stack || error);
    const artifacts = await evidence.fail(error,capture());
    console.error(browserE2eFailureFooter(artifacts)); throw error;
  } finally {
    // Leave the browser's native fullscreen before teardown, without gameplay changes.
    await actor?.evaluate('document.fullscreenElement ? document.exitFullscreen().then(()=>true) : true').catch(()=>{});
    await actor?.close?.().catch(()=>{});
    if (browser === 'firefox') await deleteSession(driver).catch(()=>{});
    await Promise.all([stopProcess(driver.child), stopProcess(server.child)]);
    if (driver.userDataDir) fs.rmSync(driver.userDataDir,{recursive:true,force:true,maxRetries:10,retryDelay:150});
    staticServer.server.close();
  }
}
main().catch(error=>{console.error(error.stack||error);process.exitCode=1;});
