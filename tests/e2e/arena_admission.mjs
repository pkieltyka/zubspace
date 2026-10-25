#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import { freePort, sleep, startRealServer, startStaticServer, stopProcess, waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { openChromeClient, startChrome, waitForChrome, waitUntil, browserE2eFailureFooter } from "./combat_replay.mjs";
import { createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";

// This fixture changes only HTTP roster metadata, never RTC gameplay packets.
// Two bots/people occupy an operator-limited arena; native tests own 499/500.
const cases = [];
const entryOnly = process.env.ZUBSPACE_E2E_ADMISSION_ENTRY_ONLY === '1';
const unavailableStorage = `for (const key of ['localStorage', 'sessionStorage'])
  Object.defineProperty(window, key, { configurable: true, get() { throw new DOMException('Storage blocked', 'SecurityError'); } });`;
const revealWitness = `(() => {
  window.__entryAudioWitness = [];
  for (const kind of ['playing', 'ended', 'error']) document.addEventListener(kind, event => {
    if (event.target?.id !== 'arena-entry-sound') return;
    window.__entryAudioWitness.push({ kind, at: performance.now(),
      currentTime: event.target.currentTime, duration: event.target.duration,
      source: event.target.currentSrc, activated: navigator.userActivation.hasBeenActive });
  }, true);
  const watch = new MutationObserver(() => {
    if (window.__admissionReveal) return;
    const overlay = document.querySelector('#loading-overlay');
    if (overlay?.classList.contains('hidden')) window.__admissionReveal = {
      at: performance.now(), lifecycle: { ...window.__zubspaceLifecycle },
      timing: { ...window.__zubspaceJoinTiming },
      x: window.__zubspaceUiMetrics?.playerX, y: window.__zubspaceUiMetrics?.playerY,
    };
  });
  watch.observe(document, { subtree: true, attributes: true, attributeFilter: ['class'] });
})();`;

async function key(send, key, code, virtual) {
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key, code, windowsVirtualKeyCode: virtual });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: virtual });
}
async function formReady(evaluate, message = null) {
  await waitUntil('editable admission form', () => evaluate(`(() => {
    const form = document.querySelector('#player-name-form'), input = document.querySelector('#player-name-input');
    return form?.classList.contains('visible') && !input.readOnly &&
      ${message ? `document.querySelector('#loading-identity-text').textContent === ${JSON.stringify(message)}` : 'true'};
  })()`), 20_000);
}
async function submit(send, evaluate, name, method = "Enter") {
  await evaluate(`document.querySelector('#player-name-input').focus(); document.querySelector('#player-name-input').select(); true`);
  await send('Input.insertText', { text: name });
  if (method === "Play") {
    const point = await evaluate(`(() => { const r = document.querySelector('#player-name-submit').getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
    await send('Input.dispatchMouseEvent', {type:'mousePressed',button:'left',clickCount:1,...point});
    await send('Input.dispatchMouseEvent', {type:'mouseReleased',button:'left',clickCount:1,...point});
  } else await key(send, 'Enter', 'Enter', 13);
}
async function silentEntryEvidence(client, method) {
  const state = await client.evaluate(`({events:window.__entryAudioWitness,
    entryAudioPresent:!!document.querySelector('#arena-entry-sound'),
    requestedRejectedCue:performance.getEntriesByType('resource').some(row=>row.name.includes('/arena-enter.wav'))})`);
  assert.equal(state.entryAudioPresent, false);
  assert.equal(state.requestedRejectedCue, false);
  assert.deepEqual(state.events, []);
  return {kind:'silent-entry-pending-confirmed-cue', method, ...state};
}
async function identity(evaluate) {
  return evaluate(`({ name: document.querySelector('#player-name-input').value,
    id: window.__zubspaceIdentity?.playerId,
    ship: window.zubspaceMenu.selectedShip, focused: document.activeElement.id,
    selected: document.querySelector('#player-name-input').selectionStart === 0 &&
      document.querySelector('#player-name-input').selectionEnd === document.querySelector('#player-name-input').value.length })`);
}
async function motionAfterReveal(client, spectator = false) {
  const witness = await client.evaluate('window.__admissionReveal');
  assert.equal(witness?.lifecycle.inGameplay, true);
  assert.equal(witness.lifecycle.gameplayStepReady, !spectator);
  assert.equal(witness.lifecycle.spectator, spectator);
  assert(witness.timing.controlsReadyAt <= witness.at);
  if (!spectator) assert(witness.timing.assignedAt <= witness.at);
  const start = await client.metricsState();
  await client.keyDown('ArrowUp');
  try {
    await waitUntil('genuine thrust works after reveal', async () => {
      const next = (await client.metricsState()).uiMetrics;
      return Math.hypot(next.playerX - start.uiMetrics.playerX, next.playerY - start.uiMetrics.playerY) >= 8 &&
        (spectator ? next.clientInputSeq === start.uiMetrics.clientInputSeq : next.clientInputSeq !== start.uiMetrics.clientInputSeq);
    }, 3000);
  } finally { await client.keyUp('ArrowUp'); }
  return witness;
}

async function main() {
  const evidence = createFocusedBrowserEvidence({ e2eScript: 'tests/e2e/arena_admission.mjs' });
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { botCount: entryOnly ? 0 : 1, env: {
    ZUBSPACE_SERVER_MAX_PLAYERS: '3', ZUBSPACE_SERVER_SIM_OBSERVE: '1',
    ZUBSPACE_SERVER_TEST_PRIZE_BASELINE_DELAY_SNAPSHOTS: entryOnly ? '0' : '600',
  } });
  const browsers = [startChrome(await freePort(), 'admission-owner'), startChrome(await freePort(), 'admission-retry')];
  let alpha, beta, replacement;
  const urls = [], screenshots = {};
  const roster = async () => {
    const listing = await (await fetch(`http://127.0.0.1:${port}/v1/players`)).json();
    assert.equal(listing.total_players, listing.players.length);
    return listing.players;
  };
  const capture = () => ({ server, serverPort: port, staticServer, urls,
    clients: [{ label: 'owner', client: alpha }, { label: 'replacement', client: replacement || beta }].filter(row => row.client),
    extraScreenshots: screenshots, extraLogs: { cases: JSON.stringify(cases), 'owner-chrome': browsers[0].logs, 'retry-chrome': browsers[1].logs },
    disconnect: async () => { for (const client of [alpha, beta, replacement]) await client?.evaluate('window.__zubspaceSendDisconnectForTest?.(); true').catch(() => {}); },
  });
  try {
    await waitForServerListening(port, server.child, server.logs);
    await Promise.all(browsers.map(waitForChrome));
    const base = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=chaos-zone&password=secret&packetlog=1&trace=1`;
    const ownerUrl = `${base}&name=AdmissionAce`; urls.push(ownerUrl);
    alpha = await openChromeClient(browsers[0], ownerUrl, { initScript: revealWitness, beforeReady: entryOnly ? null : async ({ send, evaluate }) => {
      await waitUntil('delayed baseline is assigned but controls remain closed', () => evaluate(`
        window.__zubspaceLifecycle?.arenaAssigned && !window.__zubspaceLifecycle.inGameplay &&
        window.__zubspaceUiMetrics?.localGameplayState?.networkPrizeBaselinePending === true`), 15_000);
      const pending = await evaluate(`({ lifecycle: window.__zubspaceLifecycle,
        visible: !document.querySelector('#loading-overlay').classList.contains('hidden'),
        inputSeq: window.__zubspaceUiMetrics.clientInputSeq })`);
      assert.equal(pending.visible, true);
      await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowUp', code: 'ArrowUp', windowsVirtualKeyCode: 38 });
      await sleep(250);
      await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'ArrowUp', code: 'ArrowUp', windowsVirtualKeyCode: 38 });
      assert.equal(await evaluate('window.__zubspaceUiMetrics.clientInputSeq'), pending.inputSeq);
      assert.equal(await evaluate(`document.querySelector('#loading-overlay').classList.contains('hidden')`), false);
      cases.push({ kind: 'delayed-baseline-stays-covered-with-no-gameplay-input', pending });
    } });
    cases.push({ kind: entryOnly ? 'ordinary-reveal-and-immediate-thrust' : 'delayed-reveal-and-immediate-thrust', witness: await motionAfterReveal(alpha) });
    assert.deepEqual(await alpha.evaluate('window.__entryAudioWitness'), [], 'automatic named join does not play the rejected entry cue');
    if (entryOnly) {
      const observerUrl = `${base}&name=InitialViewer`; urls.push(observerUrl);
      // Establish a real quality hold through seeded delivery impairment, then
      // rejoin with the same stored identity. The server forces the initial
      // spectator path; the fixture never writes gameplay state or packets.
      beta = await openChromeClient(browsers[1], `${observerUrl}&network_profile=degraded-latency&network_profile_seed=271828&network_profile_armed=1`, { initScript: revealWitness });
      assert.equal(await beta.evaluate('window.__zubspaceBeginNetworkProfileMeasurementForTest()'), true);
      await waitUntil('real degraded delivery establishes a spectator hold', async () => {
        const q = (await (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json()).connection_quality;
        return q.forced_total === 1 && (await beta.metricsState()).uiMetrics.localGameplayState.observerReady;
      }, 85_000);
      assert.equal(await beta.evaluate('window.__zubspaceEndNetworkProfileMeasurementForTest()'), true);
      await beta.press('Escape', 60); await beta.press('q', 60);
      await waitUntil('held identity leaves before reconnect', async () => !(await roster()).some(row => row.name === 'InitialViewer'), 10_000);
      replacement = await openChromeClient(browsers[1], observerUrl, { initScript: revealWitness });
      cases.push({ kind: 'initial-observer-reveal-and-immediate-camera-pan', serverForcedByRealQualityHold: true,
        witness: await motionAfterReveal(replacement, true) });
      console.log(JSON.stringify(await evidence.succeed({ ok: true, cases, entryOnly: true }, capture())));
      return;
    }
    const ownerId = (await alpha.metricsState()).uiMetrics.selfPlayerId;
    const bot = (await roster()).find(row => row.id !== ownerId);
    assert(bot, 'the fixture must contain a real bot');
    const retryUrl = `${base}&menu=1`; urls.push(retryUrl);
    beta = await openChromeClient(browsers[1], retryUrl, { initScript: revealWitness + unavailableStorage, beforeReady: async ({ send, evaluate }) => {
      await formReady(evaluate);
      for (const name of ['Old Name', 'bad_name', 'x.y', 'ABCDEFGHIJKLMNO']) {
        await submit(send, evaluate, name);
        await formReady(evaluate, 'Use 1–14 letters, numbers, ! or * for your player name.');
        assert.equal(await evaluate('document.querySelector("#player-name-input").value'), name);
        assert.equal(await evaluate('document.querySelector("#player-name-submit").disabled'), true);
        assert.equal(await evaluate(`performance.getEntriesByType('resource').filter(row => row.name.endsWith('/v1/rtc/offer')).length`), 0);
        cases.push({kind:'invalid-name-stays-editable-without-joining', name});
      }
      for (const [name, valid] of [
        ['\u200b\t admissionace \u00a0', true],
        [bot.name, /^[A-Za-z0-9!*]{1,14}$/.test(bot.name)],
      ]) {
        await submit(send, evaluate, name);
        // Bot display names deliberately retain the owner's spaces/dots.
        // Those cannot be entered by a human under the new strict policy;
        // valid bot-name ownership is additionally covered by native admission.
        await formReady(evaluate, valid ? 'That username is currently in use. Try another name.'
          : 'Use 1–14 letters, numbers, ! or * for your player name.');
        assert.equal((await identity(evaluate)).focused, 'player-name-input');
        assert.equal(await evaluate(`performance.getEntriesByType('resource').filter(row => row.name.endsWith('/v1/rtc/offer')).length`), 0);
        cases.push({ kind: valid ? 'advisory-name-rejection' : 'server-owned-bot-name-disallowed-as-player-input', bot: name === bot.name });
      }
      await evaluate(`document.querySelector('.ship-option[data-ship="3"]').click();
        const originalFetch = window.fetch.bind(window);
        window.fetch = (url, init) => String(url).endsWith('/v1/players')
          ? Promise.resolve(new Response('{"total_players":0,"players":[]}', { headers: { 'Content-Type': 'application/json' } })) : originalFetch(url, init); true`);
      await submit(send, evaluate, 'AdmissionAce');
      await waitUntil('identity recorded before the server rejection', () => evaluate('!!window.__zubspaceIdentity?.playerId'), 5000);
      const rejectedId = await evaluate('window.__zubspaceIdentity.playerId');
      await formReady(evaluate, 'That username is currently in use. Try another name.');
      const recovered = await identity(evaluate);
      assert.deepEqual(recovered, { name: 'AdmissionAce', id: rejectedId, ship: 3, focused: 'player-name-input', selected: true });
      assert.equal((await roster()).filter(row => row.name === 'AdmissionAce').length, 1);
      assert.equal((await roster()).find(row => row.name === 'AdmissionAce').id, ownerId);
      cases.push({ kind: 'server-name-race-recovery', preservedIdentity: true, preservedShip: true });
      await submit(send, evaluate, '\u200b\t FreshBeta!* \u00a0');
    } });
    cases.push({ kind: 'edited-name-joins', witness: await motionAfterReveal(beta) });
    assert.equal((await roster()).filter(row => row.name === 'FreshBeta!*').length, 1);
    cases.push(await silentEntryEvidence(beta, 'Enter'));
    assert.equal((await roster()).length, 3);
    const fullUrl = `${base}&menu=1`; urls.push(fullUrl);
    replacement = await openChromeClient(browsers[1], fullUrl, { initScript: revealWitness + unavailableStorage, beforeReady: async ({ send, evaluate }) => {
      await formReady(evaluate);
      await evaluate(`document.querySelector('.ship-option[data-ship="5"]').click(); true`);
      await submit(send, evaluate, 'FullRetry');
      await formReady(evaluate, 'Sorry, the arena is full at this time.');
      const preview = await identity(evaluate);
      assert.equal(preview.id, undefined, 'HTTP capacity preview must not create a join identity');
      assert.equal(preview.ship, 5);
      assert.equal(preview.focused, 'player-name-input');
      assert.equal(preview.selected, true);
      assert.equal(await evaluate(`performance.getEntriesByType('resource').filter(row => row.name.endsWith('/v1/rtc/offer')).length`), 0);
      cases.push({ kind: 'advisory-full-rejection-before-rtc', configuredLimit: 3, includesBots: true });
      // Simulate a stale HTTP count so the existing server-side full/race
      // contract is exercised too. RTC and the real server remain untouched.
      await evaluate(`const realFetch = window.fetch.bind(window);
        window.fetch = async (url, init) => {
          const response = await realFetch(url, init);
          if (!String(url).endsWith('/v1/zones/chaos-zone') || !response.ok) return response;
          const zone = await response.json();
          return new Response(JSON.stringify({ ...zone, total_players: 0 }), { headers: { 'Content-Type': 'application/json' } });
        }; true`);
      await submit(send, evaluate, 'FullRetry');
      await waitUntil('full rejection retains a private identity', () => evaluate('!!window.__zubspaceIdentity?.playerId'), 5000);
      const rejectedId = await evaluate('window.__zubspaceIdentity.playerId');
      await formReady(evaluate, 'Sorry, the arena is full at this time.');
      const recovered = await identity(evaluate);
      assert.deepEqual(recovered, { name: 'FullRetry', id: rejectedId, ship: 5, focused: 'player-name-input', selected: true });
      assert.equal((await roster()).length, 3);
      cases.push({ kind: 'server-full-recovery', configuredLimit: 3, includesBots: true, preservedIdentity: true, preservedShip: true });
      await beta.press('Escape', 60); await beta.press('q', 60);
      await waitUntil('committed leave frees the arena slot', async () => !(await roster()).some(row => row.name === 'FreshBeta!*'), 10_000);
      await submit(send, evaluate, 'FullRetry', 'Play');
    } });
    cases.push({ kind: 'retry-after-cleanup', witness: await motionAfterReveal(replacement) });
    cases.push(await silentEntryEvidence(replacement, 'Play'));
    assert.equal((await roster()).length, 3);
    screenshots.final = await replacement.screenshot();
    const result = await evidence.succeed({ ok: true, cases, limitScope: 'operator limit3 in browser; 499/500 native tests' }, capture());
    console.log(JSON.stringify(result));
  } catch (error) {
    const artifacts = await evidence.fail(error, capture());
    console.error(browserE2eFailureFooter(artifacts));
    throw error;
  } finally {
    for (const client of [alpha, beta, replacement]) await client?.close().catch(() => {});
    await Promise.all([...browsers.map(browser => stopProcess(browser.child)), stopProcess(server.child)]);
    for (const browser of browsers) fs.rmSync(browser.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}
main().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
